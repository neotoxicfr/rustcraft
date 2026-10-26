//! Survival loop of the integrated server (milestone 2): state added to the server, routing of the
//! survival packets, tick and save. The detail is in `containers`, `drops`, `vitals`,
//! `weather`; here the glue with `Server` (one-line calls from `server.rs`, `play.rs`,
//! `save.rs`).

use hashbrown::HashMap;
use std::collections::BTreeSet;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use rc_proto::{Decode, Reader};
use rc_proto_v777::container::HashedStack;

use crate::datapack::DataPack;
use crate::items::Items;
use crate::menus::{Env, Menu, Out};
use crate::netstack::NetCodec;
use crate::player::Stack;
use crate::recipes::Recipes;
use crate::rng::{Rng, Sequences};
use crate::server::Server;

/// Survival debug log (`RUSTCRAFT_S2LOG`), read once.
pub(crate) fn s2log() -> bool {
    rc_data::env_flag!("RUSTCRAFT_S2LOG")
}

/// Effects of a click to be applied by the server afterwards.
#[derive(Default)]
pub struct Pending {
    pub dropped: Vec<(Stack, bool)>,
    pub crafted: Vec<usize>,
    pub smelted: bool,
    /// S7: station effects.
    pub station: Vec<crate::stations::Ev>,
    /// S9: results taken (stack, count), grid of the last crafting.
    pub made: Vec<(Stack, i32)>,
    pub grid: Vec<Stack>,
    pub brewed: Vec<Stack>,
}

pub struct Life {
    pub pack: DataPack,
    pub items: Items,
    pub codec: NetCodec,
    pub recipes: Recipes,
    pub seqs: Sequences,
    pub rng: Rng,
    pub inv_menu: Menu,
    pub menu: Option<Menu>,
    pub counter: i32,
    pub drops: Vec<crate::drops::ItemEnt>,
    /// Entity chunks that carried items at the last save.
    pub drop_chunks: std::collections::BTreeSet<(i32, i32)>,
    pub orbs: Vec<crate::drops::Orb>,
    pub furnaces: HashMap<[i32; 3], crate::containers::Furnace>,
    pub vit: crate::vitals::Vitals,
    pub weather: crate::weather::Weather,
    /// Recipe book: known recipes and recipes to highlight.
    pub known: BTreeSet<String>,
    pub highlight: BTreeSet<String>,
    pub last_recipe: Option<usize>,
    pub pending: Pending,
    /// S7: status effects of the player, station data.
    pub fx: crate::effects::Effects,
    pub stations: crate::stations::Data,
    pub brewers: HashMap<[i32; 3], crate::brewing::Brewer>,
    /// Number of openers per container (chests, barrels, ender chests, shulker boxes).
    pub openers: HashMap<[i32; 3], i32>,
    /// Lidded container opened by the player (closed again with its menu).
    pub opened: Option<[i32; 3]>,
    /// W8a: tracked equipment and attributes sent; hits received (death messages);
    /// signs open for editing (position → authorised player).
    pub attrs: crate::attrs::EquipAttrs,
    pub combat: crate::vitals::Combat,
    pub sign_editors: HashMap<[i32; 3], u128>,
}

impl Life {
    pub(crate) fn new(
        jar: &crate::registries::Jar,
        data: &std::sync::Arc<rc_data::VersionData>,
        regs: &crate::registries::Registries,
        world: &std::path::Path,
        player: &Nbt,
    ) -> Life {
        let t = std::time::Instant::now();
        let pack = DataPack::from_jar(jar);
        let items = Items::new(data.clone(), &regs.tags);
        let recipes = Recipes::load(&pack, &items);
        let seed = crate::nbtio::read_dat(&world.join("data").join("minecraft").join("world_gen_settings.dat"))
            .ok()
            .and_then(|g| g.get("data")?.get("seed")?.as_i64())
            .unwrap_or(0);
        let mut known = BTreeSet::new();
        let mut highlight = BTreeSet::new();
        if let Some(b) = player.get("recipeBook") {
            for r in crate::nbtio::list(b.get("recipes")) {
                if let Some(s) = r.as_str() {
                    known.insert(s.to_owned());
                }
            }
            for r in crate::nbtio::list(b.get("toBeDisplayed")) {
                if let Some(s) = r.as_str() {
                    highlight.insert(s.to_owned());
                }
            }
        }
        let life = Life {
            codec: NetCodec::new(data, regs),
            // A single set of sequences for everyone (all dimensions), stored at its root.
            seqs: Sequences::load(world, seed),
            rng: Rng::from_time(),
            inv_menu: Menu::inventory(),
            menu: None,
            counter: 0,
            drops: Vec::new(),
            drop_chunks: std::collections::BTreeSet::new(),
            orbs: Vec::new(),
            furnaces: HashMap::new(),
            vit: crate::vitals::Vitals::read(player),
            weather: crate::weather::Weather::read(world),
            known,
            highlight,
            last_recipe: None,
            pending: Pending::default(),
            fx: crate::effects::Effects::read(player),
            stations: crate::stations::Data::load(&pack, &items, &recipes, regs.entries("minecraft:enchantment")),
            brewers: HashMap::new(),
            openers: HashMap::new(),
            opened: None,
            attrs: Default::default(),
            combat: Default::default(),
            sign_editors: HashMap::new(),
            pack,
            items,
            recipes,
        };
        eprintln!(
            "singleplayer: survival ready ({} recipes) in {:.0} ms",
            life.recipes.list.len(),
            t.elapsed().as_secs_f64() * 1e3
        );
        life
    }
}

/// Slots of menu 0 that equip the player: head, chest, legs, feet, off hand.
pub(crate) const EQUIP_SLOTS: [usize; 5] = [5, 6, 7, 8, 45];

/// The server as seen by a menu.
pub struct MenuEnv<'a> {
    pub items: &'a Items,
    pub recipes: &'a Recipes,
    pub codec: &'a NetCodec,
    pub creative: bool,
    pub selected: usize,
    pub pending: &'a mut Pending,
    pub last_recipe: &'a mut Option<usize>,
    /// Recipe book (`limited_crafting` rule).
    pub limited: Option<&'a BTreeSet<String>>,
    /// S7: stations (data, experience level, level random).
    pub st: &'a crate::stations::Data,
    pub xp_level: i32,
    pub rng: &'a mut Rng,
    /// S8: loaded maps (cartography table).
    pub maps: &'a crate::maps::Maps,
}

impl Env for MenuEnv<'_> {
    fn st_data(&self) -> Option<&crate::stations::Data> {
        Some(self.st)
    }
    fn xp_level(&self) -> i32 {
        self.xp_level
    }
    fn station_event(&mut self, ev: crate::stations::Ev) {
        self.pending.station.push(ev);
    }
    fn level_int(&mut self, bound: i32) -> i32 {
        self.rng.next_int_bound(bound)
    }
    fn map_info(&self, id: i32) -> Option<(bool, u8)> {
        self.maps.maps.get(&id).map(|m| (m.locked, m.scale))
    }
    fn items(&self) -> &Items {
        self.items
    }
    fn creative(&self) -> bool {
        self.creative
    }
    fn selected(&self) -> usize {
        self.selected
    }
    fn drop_item(&mut self, s: Stack, thrown: bool) {
        self.pending.dropped.push((s, thrown));
    }
    fn craft(&mut self, grid: &[Option<Stack>], w: usize) -> Option<Stack> {
        let maps = self.maps;
        let (i, s) = self.recipes.craft(self.items, grid, w, &|id| maps.scale_of(id))?;
        *self.last_recipe = Some(i);
        if let Some(known) = self.limited {
            let r = &self.recipes.list[i];
            let special = matches!(
                r.body,
                crate::recipes::Body::Craft(crate::recipes::Craft::Repair | crate::recipes::Craft::Special(_))
            );
            if !special && !known.contains(&r.id) {
                return None;
            }
        }
        Some(s)
    }
    fn crafter_result(&mut self, grid: &[Option<Stack>]) -> Option<Stack> {
        let maps = self.maps;
        self.recipes.craft(self.items, grid, 3, &|id| maps.scale_of(id)).map(|r| r.1)
    }
    fn remaining(&mut self, grid: &[Option<Stack>], _w: usize) -> Vec<Option<Stack>> {
        let maps = self.maps;
        self.recipes.remaining(self.items, grid, &|id| maps.scale_of(id))
    }
    fn crafted(&mut self, _result: &Stack, _count: i32) {
        if let Some(i) = *self.last_recipe {
            self.pending.crafted.push(i);
        }
    }
    fn smelted(&mut self, _count: i32) {
        self.pending.smelted = true;
    }
    fn made(&mut self, s: &Stack, n: i32) {
        self.pending.made.push((s.clone(), n)); // S9
    }
    fn brewed(&mut self, s: &Stack) {
        self.pending.brewed.push(s.clone()); // S9
    }
    fn craft_grid(&mut self, g: &[Option<Stack>]) {
        self.pending.grid = g.iter().flatten().cloned().collect(); // S9
    }
    fn is_fuel(&self, s: &Stack) -> bool {
        self.items.default_component(s, "minecraft:cooking_fuel").is_some()
    }
    fn can_smelt(&self, s: &Stack, kind: u8) -> bool {
        let k = match kind {
            0 => crate::recipes::CookKind::Smelting,
            1 => crate::recipes::CookKind::Blasting,
            _ => crate::recipes::CookKind::Smoking,
        };
        self.recipes.cooking(self.items, k, s).is_some()
    }
    fn hashed(&self, s: Option<&Stack>) -> Option<HashedStack> {
        normalize(self.codec.hashed(self.items, s))
    }
}

/// Hashed stack read in `container_click`.
fn read_hashed(r: &mut Reader<'_>) -> rc_proto::Result<Option<HashedStack>> {
    if !bool::decode(r)? {
        return Ok(None);
    }
    let item = read_varint(r)?;
    let count = read_varint(r)?;
    let n = read_varint(r)?.clamp(0, 256);
    let mut added = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let ty = read_varint(r)? as u16;
        added.push((ty, i32::decode(r)?));
    }
    let n = read_varint(r)?.clamp(0, 256);
    let mut removed = Vec::with_capacity(n as usize);
    for _ in 0..n {
        removed.push(read_varint(r)? as u16);
    }
    Ok(Some(HashedStack { item, count, added, removed }))
}

/// Normalised copy (sorted components) to compare with a received fingerprint.
pub fn normalize(h: Option<HashedStack>) -> Option<HashedStack> {
    h.map(|mut h| {
        h.added.sort_unstable();
        h.removed.sort_unstable();
        h
    })
}

impl Server {
    /// Menu environment (disjoint fields of the server borrowed).
    pub(crate) fn with_menu<R>(
        &mut self,
        open: bool,
        f: impl FnOnce(&mut Menu, &mut [Option<Stack>], &mut MenuEnv<'_>) -> R,
    ) -> Option<R> {
        let creative = self.game_mode == 1;
        let selected = self.held as usize;
        let limited = self.rule("minecraft:limited_crafting", false);
        let life = &mut *self.life;
        let Life { items, recipes, codec, pending, last_recipe, inv_menu, menu, known, stations, rng, vit, .. } = life;
        let mut env = MenuEnv {
            st: stations,
            xp_level: vit.xp_level,
            rng,
            items,
            recipes,
            codec,
            creative,
            selected,
            pending,
            last_recipe,
            limited: if limited { Some(known) } else { None },
            maps: &self.maps, // S8
        };
        let m = if open { menu.as_mut()? } else { inv_menu };
        Some(f(m, &mut self.slots, &mut env))
    }

    /// Crafting result recomputed after a recipe placement.
    pub(crate) fn with_menu_result(&mut self, open: bool) {
        self.with_menu(open, |m, inv, env| {
            if m.grid.is_some() {
                m.update_result(env, inv);
            }
        });
    }

    /// Current menu: the open container, otherwise the inventory.
    pub(crate) fn current_window(&self) -> i32 {
        self.life.menu.as_ref().map_or(0, |m| m.window)
    }

    /// Survival packets read before the general decoding; `true` if the packet is handled here.
    pub(crate) fn life_packet(&mut self, p: &[u8]) -> bool {
        let ids = self.peer.ids();
        let mut r = Reader::new(p);
        let Ok(id) = read_varint(&mut r) else { return false };
        if crate::life::s2log()
            && ![ids.move_pos, ids.move_pos_rot, ids.move_rot, ids.move_status, ids.client_tick_end].contains(&id)
        {
            eprintln!("singleplayer: packet {id} ({} bytes)", p.len());
        }
        let r = &mut r;
        if id == ids.container_click_out {
            let _ = self.read_click(r);
            true
        } else if id == ids.bundle_item_selected_out {
            if let (Ok(slot), Ok(index)) = (read_varint(r), read_varint(r)) {
                let open = self.life.menu.is_some();
                self.with_menu(open, |m, inv, env| m.select_bundle(env.items(), inv, slot, index));
            }
            true
        } else if id == ids.container_close_out {
            let _ = read_varint(r);
            self.close_menu(false);
            true
        } else if id == ids.client_command_out {
            match read_varint(r).ok() {
                Some(0) => self.perform_respawn(),
                Some(2) => self.send_game_rule_values(), // W8a: game rules screen
                _ => {}
            }
            true
        } else if id == ids.player_action {
            // 26.3 numbering: 0 start, 1 face change, 2 abort, 3 end of mining (milestone 1),
            // 4 drop all, 5 drop one, 6 release use, 7 swap hands, 8 stab.
            let Ok(action) = read_varint(r) else { return false };
            if !(4..=7).contains(&action) {
                return false;
            }
            if action == 6 {
                self.proj_release(); // S8: bow, trident
            }
            self.on_hand_action(action - 1);
            true
        } else if id == ids.use_item {
            let (Ok(hand), Ok(seq)) = (read_varint(r), read_varint(r)) else { return false };
            self.acknowledge(seq);
            // S8: rotation of the packet applied before use.
            if let (Ok(y), Ok(x)) = (f32::decode(r), f32::decode(r))
                && y.is_finite()
                && x.is_finite()
            {
                self.yaw = crate::mobs::mth::wrap(y);
                self.pitch = crate::mobs::mth::wrap(x);
            }
            // Demo over: use of a held item refused (reminder).
            let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
            if self.slots.get(slot).is_some_and(Option::is_some) && self.demo_refuses() {
                return true;
            }
            if !self.proj_use_item(hand)
                && !self.book_use_item(hand)
                && !self.w8c_use_item(hand)
                && !self.ray_item_use(hand)
            {
                self.on_use_item(hand);
            }
            true
        } else if id == ids.recipe_book_seen_recipe {
            // W8c: recipe seen, removed from the "new" ones.
            if let Ok(r) = read_varint(r) {
                self.book_seen(r);
            }
            true
        } else if id == ids.recipe_book_change_settings {
            // W8c: settings of a book (open, filter), kept in the player's file.
            if let (Ok(b), Ok(open), Ok(filter)) = (read_varint(r), bool::decode(r), bool::decode(r)) {
                self.book_settings(b, open, filter);
            }
            true
        } else if id == ids.place_recipe {
            if let (Ok(w), Ok(rid), Ok(max)) = (read_varint(r), read_varint(r), bool::decode(r)) {
                self.place_recipe(w, rid, max);
            }
            true
        } else if id == ids.player_command {
            // Entity, action: 0 leave bed, 1/2 start/end of sprint.
            let _ = read_varint(r);
            let action = read_varint(r);
            let data = read_varint(r).unwrap_or(0);
            match action {
                Ok(0) => self.wake_up(),
                Ok(1) => self.life.vit.sprinting = true,
                Ok(2) => self.life.vit.sprinting = false,
                Ok(6) => self.start_fall_flying(),    // S8: elytra
                Ok(a) => self.mount_command(a, data), // F04: mount jump, inventory
                _ => {}
            }
            true
        } else if id == ids.change_game_mode {
            if let Ok(m) = read_varint(r)
                && self.gamemaster()
            {
                self.set_game_mode(m);
            }
            true
        } else if id == ids.set_creative_mode_slot {
            let slot = i16::decode(r).unwrap_or(0);
            let stack = self.peer_codec_stack(r);
            self.on_creative_slot(slot, stack);
            true
        } else {
            // S7: station buttons and name; W8a: difficulty, rules, signs, books,
            // entities, F3+I.
            self.station_packet(id, r) || self.solo_packet(id, r)
        }
    }

    /// Stack with the creative "untrusted" codec: each added component prefixed with its length.
    fn peer_codec_stack(&self, r: &mut Reader<'_>) -> Option<Stack> {
        let count = read_varint(r).ok()?;
        if count <= 0 {
            return None;
        }
        let item = read_varint(r).ok()?;
        let n_add = read_varint(r).ok()?.clamp(0, 256);
        let n_rem = read_varint(r).ok()?.clamp(0, 256);
        let mut added = Vec::new();
        for _ in 0..n_add {
            let ty = read_varint(r).ok()? as u16;
            let len = rc_proto::read_len(r, 1 << 21).ok()?;
            added.push((ty, r.take(len).ok()?.to_vec().into_boxed_slice()));
        }
        let mut removed = Vec::new();
        for _ in 0..n_rem {
            removed.push(read_varint(r).ok()? as u16);
        }
        let s = rc_proto_v777::item::ItemStack { item, count, added, removed };
        self.life.codec.from_net(&self.life.items, &s)
    }

    fn read_click(&mut self, r: &mut Reader<'_>) -> rc_proto::Result<()> {
        let window = read_varint(r)?;
        let state = read_varint(r)?;
        let slot = i16::decode(r)?;
        let button = i8::decode(r)?;
        let mode = read_varint(r)?;
        let n = read_varint(r)?.clamp(0, 128);
        let mut changed = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let s = i16::decode(r)?;
            changed.push((s, normalize(read_hashed(r)?)));
        }
        let carried = normalize(read_hashed(r)?);
        if crate::life::s2log() {
            eprintln!("singleplayer: click window {window} state {state} slot {slot} button {button} mode {mode}");
        }
        self.on_click(window, state, slot, button, mode, changed, carried);
        Ok(())
    }

    /// `container_click` (§ 4.1).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn on_click(
        &mut self,
        window: i32,
        state: i32,
        slot: i16,
        button: i8,
        mode: i32,
        changed: Vec<(i16, Option<HashedStack>)>,
        carried: Option<HashedStack>,
    ) {
        if window != self.current_window() {
            return;
        }
        let open = window != 0;
        if self.game_mode == 3 || self.life.vit.dead {
            self.with_menu(open, |m, inv, _| m.send_all(inv));
            self.flush_menus();
            return;
        }
        if open && !self.menu_valid() {
            return;
        }
        // The click starts from the actual contents of the container (hopper, furnace… changed in the meantime).
        self.pull_menu();
        let len = if open { self.life.menu.as_ref().map_or(0, Menu::len) } else { self.life.inv_menu.len() };
        if !(slot == -1 || slot == -999 || (0..len as i16).contains(&slot)) {
            return;
        }
        // Armour and off hand of the inventory before the click (equipment sound and event).
        let worn_before = (!open).then(|| EQUIP_SLOTS.map(|k| self.slots[k].clone()));
        self.with_menu(open, |m, inv, env| {
            let resync = state != m.state;
            m.click(env, inv, slot, button, mode);
            for (s, h) in changed {
                m.set_remote(s, h);
            }
            m.set_remote_carried(carried);
            if resync {
                m.send_all(inv);
            } else {
                m.broadcast(env, inv);
            }
        });
        if let Some(before) = worn_before {
            self.equip_changed(&before);
        }
        self.after_menu_action();
    }

    /// Equipment slots of the inventory changed by the player: equipment reaction of
    /// each (item sound, game event), like a placement by the player in the slot.
    pub(crate) fn equip_changed(&mut self, before: &[Option<Stack>; 5]) {
        for (k, old) in EQUIP_SLOTS.into_iter().zip(before) {
            if self.slots[k] != *old {
                let new = self.slots[k].clone();
                self.on_equip_item(k, old.as_ref(), new.as_ref());
            }
        }
    }

    /// After a click or a closing: items dropped, recipes learned, furnace experience,
    /// container contents copied back, packets sent.
    pub(crate) fn after_menu_action(&mut self) {
        let p = std::mem::take(&mut self.life.pending);
        for (s, _) in p.dropped.iter().filter(|d| d.1) {
            self.s9_dropped(s); // S9: dropped from a menu
        }
        self.s9_menu_made(&p.made, &p.grid, &p.crafted, &p.station); // S9
        self.s9_brewed(&p.brewed); // S9
        for (s, thrown) in p.dropped {
            self.drop_from_player(s, thrown);
        }
        let ids: Vec<String> =
            p.crafted.iter().filter_map(|&i| self.life.recipes.list.get(i).map(|r| r.id.clone())).collect();
        self.award_recipes(&ids);
        if p.smelted {
            self.furnace_xp();
        }
        self.station_events(p.station); // S7
        self.merchant_sync(); // F05: trades made
        self.sync_container_back();
        self.flush_menus();
    }

    /// Sends the packets accumulated by the menus.
    pub(crate) fn flush_menus(&mut self) {
        let mut outs = Vec::new();
        outs.push((0, std::mem::take(&mut self.life.inv_menu.out)));
        if let Some(m) = &mut self.life.menu {
            outs.push((m.window, std::mem::take(&mut m.out)));
        }
        for (window, list) in outs {
            for o in list {
                let ev = match o {
                    Out::Slot { state, slot, stack } => rc_proto_v777::item::Inventory::Slot {
                        container: window,
                        state,
                        slot,
                        stack: self.net(stack.as_ref()),
                    },
                    Out::Content { state, slots, carried } => rc_proto_v777::item::Inventory::Content {
                        container: window,
                        state,
                        slots: slots.iter().map(|s| self.net(s.as_ref())).collect(),
                        carried: self.net(carried.as_ref()),
                    },
                    Out::Cursor(s) => rc_proto_v777::item::Inventory::Cursor(self.net(s.as_ref())),
                    Out::Data { prop, value } => {
                        let _ = self.send(Event::ContainerData { window, prop, value });
                        continue;
                    }
                };
                let _ = self.send(Event::Inventory(ev));
            }
        }
    }

    pub(crate) fn net(&self, s: Option<&Stack>) -> Option<rc_proto_v777::item::ItemStack> {
        s.and_then(|s| self.life.codec.to_net(&self.life.items, s))
    }

    /// Full sending of the inventory (entering the game, respawn).
    pub(crate) fn life_send_inventory(&mut self) {
        self.with_menu(false, |m, inv, _| m.send_all(inv));
        self.flush_menus();
    }

    /// Differences in the inventory or the open menu (each tick, after an action).
    pub(crate) fn broadcast_menus(&mut self) {
        self.pull_menu();
        let open = self.life.menu.is_some();
        self.with_menu(open, |m, inv, env| m.broadcast(env, inv));
        self.flush_menus();
    }

    /// Closing of the open menu (`forced`: by the server, `container_close` sent).
    pub(crate) fn close_menu(&mut self, forced: bool) {
        let open = self.life.menu.is_some();
        if forced && let Some(m) = &self.life.menu {
            let _ = self.send(Event::ContainerClosed);
            let _ = m;
        }
        self.with_menu(open, |m, inv, env| m.close(env, inv));
        // Lidded container closed by its last opener.
        if let Some(p) = self.life.opened.take() {
            self.container_openers(p, crate::mobs::mob::Ent::Player, -1);
        }
        if let Some(m) = self.life.menu.take() {
            // Remote copy of the player's slots taken back by menu 0.
            for (i, d) in m.slots.iter().enumerate() {
                if let crate::menus::Src::Inv(k) = d.src {
                    self.life.inv_menu.set_remote_exact(k, m.remote_of(i).clone());
                }
            }
            self.write_container(&m);
            if let Some(t) = &m.merchant {
                self.merchant_closed(t.mob); // F05
            }
            if let Some(id) = m.cart {
                self.cart_menu_closed(id); // G5-F06: chest minecart
            }
        }
        self.after_menu_action();
    }

    /// Recipes learned: added to the book and sent.
    pub(crate) fn award_recipes(&mut self, ids: &[String]) {
        // S7: special recipes do not enter the book.
        let special = |id: &str| {
            self.life.recipes.index(id).is_some_and(|i| {
                matches!(
                    self.life.recipes.list[i].body,
                    crate::recipes::Body::Craft(crate::recipes::Craft::Repair | crate::recipes::Craft::Special(_))
                )
            })
        };
        let new: Vec<String> =
            ids.iter().filter(|id| !self.life.known.contains(*id) && !special(id)).cloned().collect();
        if new.is_empty() {
            return;
        }
        for id in &new {
            self.life.known.insert(id.clone());
            self.life.highlight.insert(id.clone());
        }
        self.trig_recipes_unlocked(&new); // S9: `recipe_unlocked`
        // W8c: notification only if the recipe asks for it (`show_notification`).
        let notify = |id: &str| self.life.recipes.index(id).is_none_or(|i| self.life.recipes.list[i].notify);
        let entries = new.iter().flat_map(|id| self.book_entries(id, if notify(id) { 3 } else { 2 })).collect();
        let _ = self.send(Event::Recipes(rc_client::recipes::RecipeEvent::Add(entries, false)));
    }

    /// Book entries sent on entering the game.
    pub(crate) fn send_recipe_book(&mut self) {
        let ids: Vec<String> = self.life.known.iter().cloned().collect();
        let entries = ids
            .iter()
            .flat_map(|id| {
                let flags = if self.life.highlight.contains(id) { 2 } else { 0 };
                self.book_entries(id, flags)
            })
            .collect();
        let _ = self.send(Event::Recipes(rc_client::recipes::RecipeEvent::Add(entries, true)));
        let book = self.pdata.root.get("recipeBook");
        let flag = |k: &str| book.and_then(|b| crate::nbtio::int(b, k)).is_some_and(|v| v != 0);
        let settings = [
            (flag("isGuiOpen"), flag("isFilteringCraftable")),
            (flag("isFurnaceGuiOpen"), flag("isFurnaceFilteringCraftable")),
            (flag("isBlastingFurnaceGuiOpen"), flag("isBlastingFurnaceFilteringCraftable")),
            (flag("isSmokerGuiOpen"), flag("isSmokerFilteringCraftable")),
        ];
        let _ = self.send(Event::Recipes(rc_client::recipes::RecipeEvent::Settings(settings)));
    }

    /// Survival fields of the player's file (`players/data`).
    pub(crate) fn life_player_nbt(&self, root: &mut Nbt) {
        use crate::nbtio::set;
        self.life.vit.write(root);
        self.life.fx.write(root); // S7
        let strings = |s: &BTreeSet<String>| Nbt::List(s.iter().map(|x| Nbt::String(x.clone())).collect());
        let mut book = root.get("recipeBook").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        set(&mut book, "recipes", strings(&self.life.known));
        set(&mut book, "toBeDisplayed", strings(&self.life.highlight));
        set(root, "recipeBook", book);
    }

    /// Files specific to survival written with the save: random sequences, weather.
    pub(crate) fn life_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        self.flush_furnaces();
        self.flush_brewers(); // S7
        self.drops_save_jobs(jobs);
        self.poi_save_jobs(jobs); // F05: points of interest
        if let Some(n) = self.life.seqs.to_nbt() {
            jobs.push(crate::save::Job::Dat {
                path: self.life.seqs.path().to_owned(),
                nbt: crate::nbtio::encode_root(&n),
                backup: false,
            });
        }
        if let Some((path, n)) = self.life.weather.to_nbt(&self.world) {
            jobs.push(crate::save::Job::Dat { path, nbt: crate::nbtio::encode_root(&n), backup: false });
        }
    }

    /// Survival tick (within the server tick, after the clocks).
    pub(crate) fn life_tick(&mut self) {
        // Frozen (`tick freeze`): block entities and dropped items wait; the player ticks.
        // (Weather is advanced at the start of the world tick, in `game_tick`.)
        if self.cmd.ticking.runs() {
            self.furnaces_tick();
            self.brewers_tick(); // S7
            self.drops_tick();
        }
        self.vitals_tick();
        self.sign_editors_tick(); // W8a: sign editors too far away forgotten
        self.sync_attributes(); // W8a: attributes changed (equipment, effects, mode)
        if self.life.menu.is_some() && !self.menu_valid() {
            self.close_menu(true);
        }
        self.broadcast_menus();
    }
}
