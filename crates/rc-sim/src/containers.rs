//! Blocks with a menu (`server-gameplay.md` § 4.3-4.4, `inventories.md` § 3, 5): crafting table, chests
//! (single, double, Ender), barrels, Shulker boxes, dispensers, hoppers,
//! furnaces. Contents read and rewritten in the block entities of the chunk (Anvil format); furnaces
//! tick every tick, menu open or not.

use std::collections::BTreeMap;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use crate::items::same;
use crate::menus::{Kind, Menu};
use crate::nbtio::{int, list, set};
use crate::player::Stack;
use crate::recipes::CookKind;
use crate::server::Server;

/// Furnace (furnace 0, blast furnace 1, smoker 2) being simulated.
#[derive(Debug, Clone)]
pub struct Furnace {
    pub kind: u8,
    pub items: [Option<Stack>; 3],
    pub lit_remaining: i32,
    pub lit_total: i32,
    pub timer: i32,
    pub total: i32,
    pub speed: f32,
    pub used: BTreeMap<String, i32>,
    pub dirty: bool,
}

fn furnace_kind(id: &str) -> Option<u8> {
    match id {
        "minecraft:furnace" => Some(0),
        "minecraft:blast_furnace" => Some(1),
        "minecraft:smoker" => Some(2),
        _ => None,
    }
}

fn cook_kind(k: u8) -> CookKind {
    match k {
        0 => CookKind::Smelting,
        1 => CookKind::Blasting,
        _ => CookKind::Smoking,
    }
}

/// `Items` of a block entity → slots.
pub fn read_items(be: &Nbt, size: usize) -> Vec<Option<Stack>> {
    let mut out = vec![None; size];
    for it in list(be.get("Items")) {
        if let (Some(s), Some(st)) = (int(it, "Slot"), Stack::from_nbt(it))
            && (0..size as i64).contains(&s)
        {
            out[s as usize] = Some(st);
        }
    }
    out
}

pub fn write_items(be: &mut Nbt, items: &[Option<Stack>]) {
    let l: Vec<Nbt> =
        items.iter().enumerate().filter_map(|(i, s)| s.as_ref().map(|s| s.to_nbt(Some(i as i8)))).collect();
    set(be, "Items", Nbt::List(l));
}

impl Furnace {
    fn from_nbt(kind: u8, be: &Nbt) -> Furnace {
        let items = read_items(be, 3);
        let mut used = BTreeMap::new();
        if let Some(Nbt::Compound(f)) = be.get("RecipesUsed") {
            for (k, v) in f {
                used.insert(k.clone(), v.as_i64().unwrap_or(0) as i32);
            }
        }
        Furnace {
            kind,
            items: [items[0].clone(), items[1].clone(), items[2].clone()],
            lit_remaining: int(be, "lit_time_remaining").unwrap_or(0) as i32,
            lit_total: int(be, "lit_total_time").unwrap_or(0) as i32,
            timer: int(be, "cooking_time_spent").unwrap_or(0) as i32,
            total: int(be, "cooking_total_time").unwrap_or(0) as i32,
            speed: crate::nbtio::float(be, "speed_multiplier").unwrap_or(1.0),
            used,
            dirty: false,
        }
    }

    fn write(&self, be: &mut Nbt) {
        set(be, "cooking_time_spent", Nbt::Int(self.timer));
        set(be, "cooking_total_time", Nbt::Int(self.total));
        set(be, "lit_time_remaining", Nbt::Int(self.lit_remaining));
        set(be, "lit_total_time", Nbt::Int(self.lit_total));
        set(be, "speed_multiplier", Nbt::Float(self.speed));
        write_items(be, &self.items);
        let used = self.used.iter().map(|(k, v)| (k.clone(), Nbt::Int(*v))).collect();
        set(be, "RecipesUsed", Nbt::Compound(used));
    }

    pub fn data(&self) -> [i32; 4] {
        [self.lit_remaining, self.lit_total, self.timer, self.total]
    }

    /// Ingredient slot rewritten (slot 0): a new item (slot empty before,
    /// or different item) recomputes the total time and resets cooking to zero; a stack that
    /// grows or empties changes nothing.
    pub fn set_input(&mut self, new: Option<Stack>, recipes: &crate::recipes::Recipes, items: &crate::items::Items) {
        let changed = match (&self.items[0], &new) {
            (_, None) => false,
            (Some(a), Some(b)) => !same(a, b),
            (None, Some(_)) => true,
        };
        if changed {
            self.total = new
                .as_ref()
                .and_then(|s| recipes.cooking(items, cook_kind(self.kind), s))
                .map_or(200, |(_, c)| total_time(c.time, self.speed));
            self.timer = 0;
        }
        self.items[0] = new;
    }
}

impl Server {
    fn block_name(&self, p: [i32; 3]) -> Option<String> {
        let s = self.state_at(p)?;
        self.data.block_of(s).map(|b| b.name.clone())
    }

    pub(crate) fn prop(&self, p: [i32; 3], key: &str) -> Option<String> {
        let s = self.state_at(p)?;
        self.data.state_properties(s).find(|(k, _)| *k == key).map(|(_, v)| v.to_owned())
    }

    /// Block entity at a position.
    pub(crate) fn block_entity(&self, p: [i32; 3]) -> Option<&Nbt> {
        let c = self.chunks.get(&(p[0] >> 4, p[2] >> 4))?.as_ref()?;
        c.block_entities.iter().find(|b| {
            int(b, "x") == Some(i64::from(p[0]))
                && int(b, "y") == Some(i64::from(p[1]))
                && int(b, "z") == Some(i64::from(p[2]))
        })
    }

    /// Modifiable block entity (created if missing); the chunk becomes to-be-saved.
    pub(crate) fn block_entity_mut(&mut self, p: [i32; 3], id: &str) -> Option<&mut Nbt> {
        let c = self.chunks.get_mut(&(p[0] >> 4, p[2] >> 4))?.as_mut()?;
        c.dirty = true;
        let at = |b: &Nbt| {
            int(b, "x") == Some(i64::from(p[0]))
                && int(b, "y") == Some(i64::from(p[1]))
                && int(b, "z") == Some(i64::from(p[2]))
        };
        let i = match c.block_entities.iter().position(at) {
            Some(i) => i,
            None => {
                c.block_entities.push(Nbt::Compound(vec![
                    ("id".into(), Nbt::String(id.to_owned())),
                    ("x".into(), Nbt::Int(p[0])),
                    ("y".into(), Nbt::Int(p[1])),
                    ("z".into(), Nbt::Int(p[2])),
                ]));
                c.block_entities.len() - 1
            }
        };
        c.block_entities.get_mut(i)
    }

    /// Right click: bed or menu block; true if the action is consumed.
    pub(crate) fn use_block_or_bed(&mut self, pos: [i32; 3]) -> bool {
        if self.block_name(pos).is_some_and(|n| n.ends_with("_bed")) {
            return self.game_mode != 3 && self.use_bed(pos);
        }
        self.use_block(pos)
    }

    /// Right click on a menu block: true if the action is consumed.
    pub(crate) fn use_block(&mut self, pos: [i32; 3]) -> bool {
        let Some(name) = self.block_name(pos) else { return false };
        let short = name.trim_start_matches("minecraft:").to_owned();
        let title = |key: &str| Some(rc_text::Component::translate(key, Vec::new()));
        let custom = self.block_entity(pos).and_then(|b| b.get("CustomName")).and_then(rc_text::from_nbt);
        // Chests (trapped and copper included): block above, halves, double title.
        let chest = matches!(short.as_str(), "chest" | "trapped_chest") || short.ends_with("copper_chest");
        // Openings refused before any menu (no window id consumed, no lock or
        // loot): chest under a conductive block or a sitting cat, Ender chest under a
        // conductive block, closed Shulker box whose open lid would hit something.
        let blocked = if chest {
            self.chest_blocked(pos)
        } else if short == "ender_chest" {
            self.v().conductor(self.get([pos[0], pos[1] + 1, pos[2]]))
        } else {
            // The spectator opens the box through its block entity, without a lid test.
            short.ends_with("shulker_box") && self.game_mode != 3 && !self.shulker_can_open(pos)
        };
        if blocked {
            return true;
        }
        let window = {
            self.life.counter = self.life.counter % 100 + 1;
            self.life.counter
        };
        // Container lock (except chests, checked after the block above).
        if !chest
            && lockable(&short)
            && !self.lock_allows(&[pos], custom.clone().or_else(|| title(default_title(&short))))
        {
            // The beacon counts the interaction even when locked.
            if short == "beacon" {
                self.award_custom("interact_with_beacon", 1);
            }
            return true;
        }
        // S7: loot of the other sealed containers.
        if !chest
            && crate::lootchest::loot_size(&name).or_else(|| short.ends_with("shulker_box").then_some(27)).is_some()
            && self.loot_before_open(&[pos])
        {
            return true;
        }
        let (menu, t) = match short.as_str() {
            "crafting_table" => (Menu::crafting(window), title("container.crafting")),
            _ if chest => {
                // Lock: each half must open (own name of a half, otherwise "double
                // chest").
                let halves = self.chest_pair(pos).map_or_else(|| vec![pos], |(a, b)| vec![a, b]);
                let named = halves
                    .iter()
                    .find_map(|&h| self.block_entity(h).and_then(|b| b.get("CustomName")).and_then(rc_text::from_nbt));
                let key = if halves.len() == 2 { "container.chestDouble" } else { "container.chest" };
                if !self.lock_allows(&halves, named.or_else(|| title(key))) {
                    return true;
                }
                // S7: loot of sealed chests (both halves of a double chest).
                if self.loot_before_open(&halves) {
                    return true;
                }
                match self.chest_pair(pos) {
                    Some((first, second)) => {
                        let mut items = self.container_items(first, 27);
                        items.extend(self.container_items(second, 27));
                        let mut m = Menu::container(window, 5, items, false);
                        m.block = Some(first);
                        m.block2 = Some(second);
                        (m, title("container.chestDouble"))
                    }
                    None => {
                        (Menu::container(window, 2, self.container_items(pos, 27), false), title("container.chest"))
                    }
                }
            }
            "barrel" => (Menu::container(window, 2, self.container_items(pos, 27), false), title("container.barrel")),
            "ender_chest" => {
                // The open chest is retained: the menu closes if it disappears or moves away.
                let mut m = Menu::container(window, 2, self.life.vit.ender.clone(), false);
                m.ender = true;
                m.block = Some(pos);
                (m, title("container.enderchest"))
            }
            s if s.ends_with("shulker_box") => {
                (Menu::container(window, 20, self.container_items(pos, 27), true), title("container.shulkerBox"))
            }
            "dispenser" => {
                (Menu::container(window, 6, self.container_items(pos, 9), false), title("container.dispenser"))
            }
            "dropper" => (Menu::container(window, 6, self.container_items(pos, 9), false), title("container.dropper")),
            "hopper" => (Menu::container(window, 16, self.container_items(pos, 5), false), title("container.hopper")),
            "furnace" | "blast_furnace" | "smoker" => {
                let kind = furnace_kind(&name).unwrap_or(0);
                self.ensure_furnace(pos, kind);
                let f = self.life.furnaces.get(&pos).cloned();
                let mut m = Menu::furnace(window, kind, f.as_ref().map(|f| f.items.to_vec()).unwrap_or_default());
                if let Some(f) = f {
                    m.data = f.data().to_vec();
                }
                let key = match kind {
                    0 => "container.furnace",
                    1 => "container.blast_furnace",
                    _ => "container.smoker",
                };
                (m, title(key))
            }
            // G5-F11: beacon and crafter; without a block entity, the click is taken without a menu.
            "beacon" => match self.beacon_menu(pos, window) {
                Some(m) => (m, title("container.beacon")),
                None => return true,
            },
            "crafter" => match self.crafter_menu(pos, window) {
                Some(m) => (m, title("container.crafter")),
                None => return true,
            },
            // S7: brewing stand, enchanting table, anvil, grindstone, smithing table, stonecutter.
            _ => match self.station_menu(&short, pos, window) {
                Some(x) => x,
                None => return false,
            },
        };
        let mut menu = menu;
        if menu.block.is_none() {
            menu.block = Some(pos);
        }
        // Container with a lid: opener counted (sound, lid, event) before the screen; the
        // spectator does not count.
        let lidded = short.ends_with("chest") || short == "barrel" || short.ends_with("shulker_box");
        let opened = lidded && self.game_mode != 3;
        self.open_menu(menu, custom.or(t));
        if opened && self.life.menu.is_some() {
            self.container_openers(pos, crate::mobs::mob::Ent::Player, 1);
            self.life.opened = Some(pos);
        }
        true
    }

    /// Shulker box at `p` openable: lid already moving or open, or half-block
    /// in front of its face (where the lid rises, shrunk by 1e-6) free of any block collision
    /// and of solid entities (the border does not count). Simulated solid entities: living
    /// shulker, living immobilized adult happy ghast; boats do not exist yet on the
    /// server side.
    pub(crate) fn shulker_can_open(&self, p: [i32; 3]) -> bool {
        if self.bs.shulkers.get(&p).is_some_and(|e| e.1 != 0) {
            return true;
        }
        let step: [i32; 3] = match self.prop(p, "facing").as_deref() {
            Some("down") => [0, -1, 0],
            Some("north") => [0, 0, -1],
            Some("south") => [0, 0, 1],
            Some("west") => [-1, 0, 0],
            Some("east") => [1, 0, 0],
            _ => [0, 1, 0],
        };
        // Unit box at the bottom center of the block, extended by half a block towards the face, then
        // stripped of its original part.
        let base = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let mut min = [-0.5, 0.0, -0.5];
        let mut max = [0.5, 1.0, 0.5];
        for a in 0..3 {
            match step[a] {
                1 => (min[a], max[a]) = (max[a], max[a] + 0.5),
                -1 => (min[a], max[a]) = (min[a] - 0.5, min[a]),
                _ => {}
            }
        }
        let e = 1.0e-6;
        let b = rc_physics::Aabb {
            min: std::array::from_fn(|a| base[a] + min[a] + e),
            max: std::array::from_fn(|a| base[a] + max[a] - e),
        };
        let w = self.world();
        let mut boxes = Vec::new();
        rc_physics::collide::collect(&w, &b, &mut boxes);
        if boxes.iter().any(|s| s.intersects(&b)) {
            return false;
        }
        // Solid entities touching the zone inflated by 1e-7 (no mob is a spectator).
        let g = 1.0e-7;
        let area = rc_physics::Aabb { min: b.min.map(|v| v - g), max: b.max.map(|v| v + g) };
        use crate::mobs::species::Kind;
        !self.mobs.list.iter().flatten().any(|m| {
            let solid = match m.kind {
                Kind::Shulker => m.alive(),
                Kind::HappyGhast => {
                    !m.is_baby() && m.alive() && crate::mobs::recent::happy_ghast::on_still_timeout(m)
                }
                _ => false,
            };
            solid && m.bbox().intersects(&area)
        })
    }

    /// Chest (single, trapped, copper) that does not open and that the comparator reads as empty:
    /// conductive block or sitting cat above it, or above its paired half (same
    /// block, opposite half, same orientation).
    pub(crate) fn chest_blocked(&self, p: [i32; 3]) -> bool {
        if self.chest_blocked_at(p) {
            return true;
        }
        let v = self.v();
        let s = self.get(p);
        let ty = v.get(s, "type").unwrap_or("single");
        if ty == "single" {
            return false;
        }
        let f = v.facing(s);
        let d = if ty == "left" { f.clockwise() } else { f.counter_clockwise() };
        let q = crate::blocks::rel(p, d);
        let t = self.get(q);
        let tt = v.get(t, "type").unwrap_or("single");
        v.block(t) == v.block(s) && tt != "single" && tt != ty && v.facing(t) == f && self.chest_blocked_at(q)
    }

    /// A single half: conductive block above, or cat in the sitting pose (order to
    /// sit or sitting on the chest) whose box intersects the cube above.
    pub(crate) fn chest_blocked_at(&self, q: [i32; 3]) -> bool {
        let up = [q[0], q[1] + 1, q[2]];
        if self.v().conductor(self.get(up)) {
            return true;
        }
        let area = rc_physics::Aabb {
            min: [f64::from(q[0]), f64::from(q[1] + 1), f64::from(q[2])],
            max: [f64::from(q[0] + 1), f64::from(q[1] + 2), f64::from(q[2] + 1)],
        };
        // Index per species: only cats are traversed (same result as the whole list).
        self.mobs
            .of_kind(crate::mobs::species::Kind::Cat)
            .any(|m| !m.removed && (m.sitting || m.cat_sit_pose) && m.touches(&area))
    }

    /// Double chest: (top half of the menu, bottom half).
    fn chest_pair(&self, pos: [i32; 3]) -> Option<([i32; 3], [i32; 3])> {
        let ty = self.prop(pos, "type")?;
        let facing = self.prop(pos, "facing")?;
        // Clockwise from the chest's gaze direction.
        let cw = |f: &str| match f {
            "north" => [1, 0, 0],
            "east" => [0, 0, 1],
            "south" => [-1, 0, 0],
            _ => [0, 0, -1],
        };
        let d = cw(&facing);
        let (dx, dz) = match ty.as_str() {
            "left" => (d[0], d[2]),
            "right" => (-d[0], -d[2]),
            _ => return None,
        };
        let other = [pos[0] + dx, pos[1], pos[2] + dz];
        if self.block_name(other) != self.block_name(pos) {
            return None;
        }
        // The "right" half comes first.
        if ty == "right" { Some((pos, other)) } else { Some((other, pos)) }
    }

    fn container_items(&self, pos: [i32; 3], size: usize) -> Vec<Option<Stack>> {
        self.block_entity(pos).map_or_else(|| vec![None; size], |b| read_items(b, size))
    }

    /// Opening: the old menu is closed, `open_screen` then full contents.
    pub(crate) fn open_menu(&mut self, menu: Menu, title: Option<rc_text::Component>) {
        if self.life.menu.is_some() {
            self.close_menu(true);
        }
        let _ = self.send(Event::OpenScreen { window: menu.window, kind: menu.menu_type(), title });
        self.life.menu = Some(menu);
        if let Some(m) = &mut self.life.menu {
            m.send_all(&self.slots);
        }
        self.flush_menus();
    }

    /// Menu still valid: block of the right type and player within range + 4.
    pub(crate) fn menu_valid(&self) -> bool {
        let Some(m) = &self.life.menu else { return true };
        if m.merchant.is_some() {
            return self.merchant_valid(); // F05
        }
        if let Some(id) = m.cart {
            return self.cart_menu_valid(id); // G5-F06: minecart with chest or hopper
        }
        let Some(p) = m.block else { return true };
        let ok_block = match m.kind {
            Kind::Crafting => self.block_name(p).as_deref() == Some("minecraft:crafting_table"),
            // G6-F01: lectern still there with a readable book.
            Kind::Container { menu_type: crate::blocks::lectern::MENU_TYPE, .. } => self.lectern_menu_valid(p),
            Kind::Furnace(k) => self.block_name(p).and_then(|n| furnace_kind(&n)) == Some(k),
            Kind::Container { .. } if m.ender => self.block_name(p).as_deref() == Some("minecraft:ender_chest"),
            Kind::Container { .. } => self.block_entity(p).is_some(),
            Kind::Inventory | Kind::Mount { .. } | Kind::Merchant => true,
            Kind::Station(st) => self.station_block_ok(st, p), // S7
            // G5-F11: beacon still there; crafter with its block entity.
            Kind::Beacon => self.block_name(p).as_deref() == Some("minecraft:beacon"),
            Kind::Crafter => {
                self.block_name(p).as_deref() == Some("minecraft:crafter") && self.block_entity(p).is_some()
            }
        };
        // Interaction range (attribute) + 4, from the eyes.
        ok_block && self.within_block_range(p, 4.0)
    }

    /// Lock (`lock`) of the containers `positions`: the main hand must satisfy each
    /// predicate (the spectator passes). Refusal: "locked" message (container name) and sound
    /// at the center; the spectator of a double chest gets neither. True if opened.
    fn lock_allows(&mut self, positions: &[[i32; 3]], name: Option<rc_text::Component>) -> bool {
        if self.game_mode == 3 {
            return true;
        }
        let held = self.slots.get(36 + self.held as usize).cloned().flatten();
        let open = positions.iter().all(|&p| match self.block_entity(p).and_then(|b| b.get("lock")) {
            None => true,
            Some(lock) => {
                let pred = crate::commands::cmds_s9b::nbt_to_json(lock);
                self.p_item(&pred, held.as_ref())
            }
        });
        if !open {
            let n = positions.len() as f64;
            let at = std::array::from_fn(|a| positions.iter().map(|p| f64::from(p[a]) + 0.5).sum::<f64>() / n);
            let args = name.map(rc_text::Arg::Component).into_iter().collect();
            let _ = self.send(Event::ActionBar(rc_text::Component::translate("container.isLocked", args)));
            self.play_sound("minecraft:block.chest.locked", 4, at, 1.0, 1.0);
        }
        open
    }

    /// Own contents of the open menu copied back to its owner (block entity, furnace, Ender).
    pub(crate) fn sync_container_back(&mut self) {
        let Some(m) = &mut self.life.menu else { return };
        if !m.own_dirty {
            return;
        }
        m.own_dirty = false;
        let own = m.own.clone();
        let (kind, block, block2, ender, cart) = (m.kind, m.block, m.block2, m.ender, m.cart);
        match kind {
            Kind::Container { .. } if cart.is_some() => {
                if let Some(c) = cart.and_then(|id| self.cart_mut(id)) {
                    let n = c.items.len().min(own.len());
                    c.items[..n].clone_from_slice(&own[..n]);
                }
            }
            // G6-F01: lectern book taken by a click in its slot.
            Kind::Container { menu_type: crate::blocks::lectern::MENU_TYPE, .. } => {
                if let Some(b) = block
                    && own.first().is_some_and(Option::is_none)
                {
                    let _ = self.lectern_take_book(b);
                }
            }
            Kind::Container { size, .. } => {
                if ender {
                    self.life.vit.ender = own;
                    return;
                }
                // Written to the existing block entity (never created here: a block broken with the menu open
                // must not be reborn as a block entity), then the "to be saved" mark: comparators.
                let halves = [(block, 0, size.min(27)), (block2, 27, own.len())];
                for (b, from, to) in halves {
                    let Some(b) = b else { continue };
                    let part = &own[from.min(own.len())..to.min(own.len())];
                    if let Some(be) = self.be_get_mut(b) {
                        write_items(be, part);
                        self.container_changed(b);
                    }
                }
            }
            Kind::Furnace(_) => {
                let Some(p) = block else { return };
                let recipes = &self.life.recipes;
                let items = &self.life.items;
                if let Some(f) = self.life.furnaces.get_mut(&p) {
                    f.set_input(own[0].clone(), recipes, items);
                    (f.items[1], f.items[2]) = (own[1].clone(), own[2].clone());
                    f.dirty = true;
                }
                self.container_changed(p);
            }
            Kind::Station(crate::stations::St::Brewing) => {
                if let Some(p) = block {
                    self.brewer_sync_back(p, &own); // S7
                    self.container_changed(p);
                }
            }
            // G5-F11: crafter grid (the shown result is not in the block entity).
            Kind::Crafter => {
                if let Some(p) = block
                    && let Some(be) = self.be_get_mut(p)
                {
                    write_items(be, &own[..9.min(own.len())]);
                    self.container_changed(p);
                }
            }
            // F04: saddle, body and chest copied back to the mount.
            Kind::Mount { entity, .. } => self.mount_menu_back(entity, &own),
            _ => {}
        }
    }

    /// Closing: final contents written (the menu is already removed).
    pub(crate) fn write_container(&mut self, m: &Menu) {
        if m.own.is_empty() || matches!(m.kind, Kind::Crafting | Kind::Inventory) {
            return;
        }
        let saved = self.life.menu.replace(Menu::container(m.window, 0, Vec::new(), false));
        if let Some(cur) = &mut self.life.menu {
            cur.kind = m.kind;
            cur.own = m.own.clone();
            cur.block = m.block;
            cur.block2 = m.block2;
            cur.ender = m.ender;
            cur.cart = m.cart;
            cur.own_dirty = true;
        }
        self.sync_container_back();
        self.life.menu = saved;
    }

    /// Furnace at `pos` simulated from the creation of its block entity (type read from the block).
    pub(crate) fn ensure_furnace_at(&mut self, pos: [i32; 3]) {
        if let Some(k) = self.block_name(pos).as_deref().and_then(furnace_kind) {
            self.ensure_furnace(pos, k);
        }
    }

    /// Open menu realigned on the source of its contents (block entity, furnace, brewing stand, Ender
    /// chest) before each click and each send: a change coming from elsewhere (hopper,
    /// dispenser, command) shows up on the client, and the next click starts from the real contents
    /// instead of overwriting the source with a stale copy.
    pub(crate) fn pull_menu(&mut self) {
        // G5-F11: live data of the beacon (levels, effects) and of the crafter (disabled slots,
        // signal); crafter grid re-read (hoppers), shown result recomputed.
        if let Some((kind, Some(p))) = self.life.menu.as_ref().map(|m| (m.kind, m.block)) {
            match kind {
                Kind::Beacon => {
                    let d = self.beacon_data(p).to_vec();
                    if let Some(m) = &mut self.life.menu {
                        m.data = d;
                    }
                }
                Kind::Crafter => {
                    let d = self.crafter_data(p);
                    let dirty = self.life.menu.as_ref().is_some_and(|m| m.own_dirty);
                    let grid = (!dirty).then(|| self.source_stacks(p)).filter(|g| g.len() == 9);
                    let stale = grid.as_ref().zip(self.life.menu.as_ref()).is_some_and(|(g, m)| m.own[..9] != g[..]);
                    let result = grid.as_ref().filter(|_| stale).map(|g| self.crafter_result(g));
                    if let Some(m) = &mut self.life.menu {
                        m.data = d;
                        if let (Some(g), Some(r)) = (grid, result) {
                            m.own[..9].clone_from_slice(&g);
                            m.own[9] = r;
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        // Menu change not yet copied back to the source: the menu's copy is authoritative.
        let Some(m) = self.life.menu.as_ref().filter(|m| !m.own_dirty) else { return };
        let fresh: Option<Vec<Option<Stack>>> = match m.kind {
            Kind::Container { .. } if m.ender => Some(self.life.vit.ender.clone()),
            Kind::Container { .. } => match (m.block, m.block2) {
                (Some(a), Some(b)) => {
                    let mut v = self.source_stacks(a);
                    v.extend(self.source_stacks(b));
                    Some(v)
                }
                (Some(a), None) => Some(self.source_stacks(a)),
                _ => None,
            },
            Kind::Furnace(_) | Kind::Station(crate::stations::St::Brewing) => {
                m.block.and_then(|p| self.live_items(p)).map(<[Option<Stack>]>::to_vec)
            }
            _ => None,
        };
        if let Some(v) = fresh
            && let Some(m) = &mut self.life.menu
            && v.len() == m.own.len()
            && v != m.own
        {
            m.own = v;
        }
    }

    /// Contents of a container with a block entity, as stacks (empty if the block entity has disappeared).
    fn source_stacks(&self, p: [i32; 3]) -> Vec<Option<Stack>> {
        if self.block_entity(p).is_none() {
            return Vec::new();
        }
        self.slots(p).iter().map(|it| it.as_ref().and_then(Stack::from_nbt)).collect()
    }

    /// Furnaces of the chunk loaded to be simulated.
    fn ensure_furnace(&mut self, pos: [i32; 3], kind: u8) {
        if self.life.furnaces.contains_key(&pos) {
            return;
        }
        let f = match self.block_entity(pos) {
            Some(be) => Furnace::from_nbt(kind, be),
            None => Furnace::from_nbt(kind, &Nbt::Compound(Vec::new())),
        };
        self.life.furnaces.insert(pos, f);
    }

    /// Furnaces found in chunks that have just been loaded.
    pub(crate) fn discover_furnaces(&mut self, loaded: &[(i32, i32)]) {
        let mut found = Vec::new();
        for p in loaded {
            let Some(Some(c)) = self.chunks.get(p) else { continue };
            for be in &c.block_entities {
                let Some(k) = be.get("id").and_then(Nbt::as_str).and_then(furnace_kind) else { continue };
                let (Some(x), Some(y), Some(z)) = (int(be, "x"), int(be, "y"), int(be, "z")) else { continue };
                found.push(([x as i32, y as i32, z as i32], k));
            }
        }
        for (p, k) in found {
            self.ensure_furnace(p, k);
        }
        self.discover_brewers(loaded); // S7
        self.adopt_items(loaded);
    }

    /// Furnace tick.
    pub(crate) fn furnaces_tick(&mut self) {
        let positions: Vec<[i32; 3]> = self.life.furnaces.keys().copied().collect();
        for p in positions {
            // Same test as the other block entities: border and chunk within tick range.
            if self.be_ticking(p) {
                self.furnace_tick(p);
            }
        }
        // Data of the open menu (its contents are re-read by `pull_menu`).
        if let Some(m) = &mut self.life.menu
            && let (Kind::Furnace(_), Some(p)) = (m.kind, m.block)
            && let Some(f) = self.life.furnaces.get(&p)
        {
            m.data = f.data().to_vec();
        }
    }

    fn furnace_tick(&mut self, p: [i32; 3]) {
        let Some(mut f) = self.life.furnaces.remove(&p) else { return };
        let mut changed = false;
        let was_lit = f.lit_remaining > 0;
        let mut lit = false;
        if was_lit {
            f.lit_remaining -= 1;
            lit = f.lit_remaining > 0;
        }
        let has_ing = f.items[0].is_some();
        let has_fuel = f.items[1].is_some();
        if lit || (has_fuel && has_ing) {
            if let Some(ing) = f.items[0].clone() {
                let recipe =
                    self.life.recipes.cooking(&self.life.items, cook_kind(f.kind), &ing).map(|(i, c)| (i, c.clone()));
                if let Some((ri, recipe)) = recipe {
                    let result = recipe.result.stack();
                    let max = self.life.items.max_stack(&result).min(99);
                    let can_burn = match &f.items[2] {
                        None => true,
                        Some(r) => same(r, &result) && r.count + result.count <= max,
                    };
                    if can_burn {
                        if !lit {
                            let fuel = f.items[1].clone();
                            let (burn, speed) = self.fuel_values(fuel.as_ref(), f.kind);
                            f.lit_remaining = burn;
                            f.lit_total = burn;
                            f.speed = speed;
                            if f.total > 0 && f.timer < f.total {
                                let ratio = f.timer as f32 / f.total as f32;
                                f.total = total_time(recipe.time, f.speed);
                                f.timer = (ratio * f.total as f32).ceil() as i32;
                            }
                            if burn > 0 {
                                if let Some(mut fuel) = fuel {
                                    let rem = self.life.items.remainder(&fuel.id);
                                    fuel.count -= 1;
                                    f.items[1] = if fuel.count > 0 {
                                        if let Some(r) = rem {
                                            self.spawn_item_at([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])], r);
                                        }
                                        Some(fuel)
                                    } else {
                                        rem
                                    };
                                }
                                lit = true;
                                changed = true;
                            }
                        }
                        if lit {
                            f.timer += 1;
                            if f.timer >= f.total {
                                f.timer = 0;
                                f.total = total_time(recipe.time, f.speed);
                                match &mut f.items[2] {
                                    Some(r) => r.count += result.count,
                                    slot @ None => *slot = Some(result),
                                }
                                if ing.id == "minecraft:wet_sponge"
                                    && f.items[1].as_ref().is_some_and(|s| s.id == "minecraft:bucket")
                                {
                                    f.items[1] =
                                        Some(Stack { id: "minecraft:water_bucket".into(), count: 1, components: None });
                                }
                                if let Some(i) = &mut f.items[0] {
                                    i.count -= 1;
                                    if i.count <= 0 {
                                        f.items[0] = None;
                                    }
                                }
                                let id = self.life.recipes.list[ri].id.clone();
                                *f.used.entry(id).or_insert(0) += 1;
                                changed = true;
                            }
                        } else {
                            f.timer = 0;
                        }
                    } else {
                        f.timer = 0;
                    }
                }
            } else {
                f.timer = 0;
            }
        } else if f.timer > 0 {
            f.timer = (f.timer - 2).clamp(0, f.total.max(0));
        }
        if was_lit != lit {
            changed = true;
            if let Some(s) = self.state_at(p)
                && let Some(new) = self.with_prop(s, "lit", if lit { "true" } else { "false" })
            {
                self.set_update(p, new); // flags 3 like the game
            }
        }
        if changed {
            f.dirty = true;
        }
        self.life.furnaces.insert(p, f);
        // The "to be saved" mark of the tick: neighbouring comparators.
        if changed {
            self.container_changed(p);
        }
    }

    /// State with one property changed.
    pub(crate) fn with_prop(&self, state: u32, key: &str, value: &str) -> Option<u32> {
        let b = self.data.block_of(state)?;
        let props: Vec<(String, Nbt)> = self
            .data
            .state_properties(state)
            .map(|(k, v)| (k.to_owned(), Nbt::String(if k == key { value.to_owned() } else { v.to_owned() })))
            .collect();
        let entry = Nbt::Compound(vec![
            ("id".into(), Nbt::String(b.name.clone())),
            ("properties".into(), Nbt::Compound(props)),
        ]);
        self.states.state_of(&self.data, &entry)
    }

    /// Burn time and multiplier of the fuel (`cooking_fuel` component).
    fn fuel_values(&mut self, fuel: Option<&Stack>, kind: u8) -> (i32, f32) {
        let Some(fuel) = fuel else { return (0, 1.0) };
        let Some(c) = self.life.items.default_component(fuel, "minecraft:cooking_fuel") else { return (0, 1.0) };
        let block = match kind {
            0 => "minecraft:furnace",
            1 => "minecraft:blast_furnace",
            _ => "minecraft:smoker",
        };
        let tags = &self.block_tags;
        let data = &self.data;
        let tag_fn = |tag: &str, name: &str| {
            let id = data.blocks.iter().position(|b| b.name == name).map(|i| i as u32);
            id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
        };
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &tag_fn };
        let ctx = crate::loot::Ctx { block: Some((block, Vec::new())), ..Default::default() };
        let mut rng = self.life.rng;
        let burn = loot.int(&c["burn_time"], &ctx, &mut rng);
        let speed =
            if c["speed_multiplier"].is_null() { 1.0 } else { loot.float(&c["speed_multiplier"], &ctx, &mut rng) };
        self.life.rng = rng;
        (burn, speed)
    }

    /// Experience of the cooked recipes given back to the player who takes the result.
    pub(crate) fn furnace_xp(&mut self) {
        let Some(p) = self.life.menu.as_ref().and_then(|m| m.block) else { return };
        let Some(f) = self.life.furnaces.get_mut(&p) else { return };
        let used = std::mem::take(&mut f.used);
        f.dirty = true;
        let mut total = 0;
        let mut ids = Vec::new();
        for (id, n) in used {
            let Some(i) = self.life.recipes.index(&id) else { continue };
            ids.push(id);
            if let crate::recipes::Body::Cook(c) = &self.life.recipes.list[i].body {
                let x = n as f32 * c.xp;
                let mut reward = x.floor() as i32;
                let frac = x - x.floor();
                if frac != 0.0 && self.life.rng.next_float() < frac {
                    reward += 1;
                }
                total += reward;
            }
        }
        self.award_recipes(&ids);
        if total > 0 {
            self.spawn_orbs(self.pos, total);
        }
    }

    /// Modified furnaces rewritten into their block entity (before saving).
    pub(crate) fn flush_furnaces(&mut self) {
        let dirty: Vec<([i32; 3], Furnace)> = self
            .life
            .furnaces
            .iter_mut()
            .filter(|(_, f)| f.dirty)
            .map(|(p, f)| {
                f.dirty = false;
                (*p, f.clone())
            })
            .collect();
        for (p, f) in dirty {
            let id = match f.kind {
                0 => "minecraft:furnace",
                1 => "minecraft:blast_furnace",
                _ => "minecraft:smoker",
            };
            if let Some(be) = self.block_entity_mut(p, id) {
                f.write(be);
            }
        }
    }

    /// Block removed: furnace forgotten, container contents thrown.
    pub(crate) fn container_broken(&mut self, p: [i32; 3]) {
        if self.brewer_broken(p) {
            return; // S7
        }
        self.unpack_loot(p, false); // S7: loot chest never opened
        if let Some(f) = self.life.furnaces.remove(&p) {
            let at = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
            for s in f.items.into_iter().flatten() {
                self.drop_contents(at, s);
            }
            // Experience of the remaining cooked recipes.
            let mut total = 0.0f32;
            for (id, n) in &f.used {
                if let Some(i) = self.life.recipes.index(id)
                    && let crate::recipes::Body::Cook(c) = &self.life.recipes.list[i].body
                {
                    total += *n as f32 * c.xp;
                }
            }
            let mut reward = total.floor() as i32;
            if total.fract() != 0.0 && self.life.rng.next_float() < total.fract() {
                reward += 1;
            }
            if reward > 0 {
                self.spawn_orbs([at[0] + 0.5, at[1] + 0.5, at[2] + 0.5], reward);
            }
            return;
        }
        let Some(be) = self.block_entity(p) else { return };
        let id = be.get("id").and_then(Nbt::as_str).unwrap_or("");
        if id.ends_with("shulker_box") {
            return;
        }
        let items: Vec<Stack> = list(be.get("Items")).iter().filter_map(Stack::from_nbt).collect();
        let at = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        for s in items {
            self.drop_contents(at, s);
        }
    }

    /// Opener who arrives (`delta` = 1) or leaves (−1) on a container with a lid: chest
    /// (both halves of a double chest), trapped chest, copper chest, barrel, Ender
    /// chest, Shulker box. The first opener and the departure of the last play the sound (for
    /// everyone, the opening player included) and emit `container_open` or `container_close`.
    pub(crate) fn container_openers(&mut self, p: [i32; 3], by: crate::mobs::mob::Ent, delta: i32) {
        let name = self.block_name(p).unwrap_or_default();
        let short = name.trim_start_matches("minecraft:");
        let halves = if short.ends_with("chest") && short != "ender_chest" {
            self.chest_pair(p).map_or_else(|| vec![p], |(a, b)| vec![a, b])
        } else {
            vec![p]
        };
        for h in halves {
            self.container_opener_one(h, by, delta);
        }
    }

    fn container_opener_one(&mut self, p: [i32; 3], by: crate::mobs::mob::Ent, delta: i32) {
        let st = self.st(p);
        let name = self.state_name(st).trim_start_matches("minecraft:").to_owned();
        let lidded = name.ends_with("chest") || name == "barrel" || name.ends_with("shulker_box");
        if !lidded {
            return;
        }
        let block = self.v().block(st);
        let prev = self.life.openers.get(&p).copied().unwrap_or(0);
        let cur = (prev + delta).max(0);
        if cur == prev {
            return;
        }
        if cur == 0 {
            self.life.openers.remove(&p);
        } else {
            self.life.openers.insert(p, cur);
        }
        let edge = (prev == 0) != (cur == 0);
        let open = cur > 0;
        let center = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let event = if open { "minecraft:container_open" } else { "minecraft:container_close" };
        let verb = if open { "open" } else { "close" };
        if name.ends_with("shulker_box") {
            // Lid first, then event and sound for the first or the last.
            self.block_event(p, block, 1, cur);
            if edge {
                self.game_event_block(event, p, Some(by), None);
                let pitch = self.mobs.rng.next_float() * 0.1 + 0.9;
                self.world_sound(&format!("minecraft:block.shulker_box.{verb}"), 4, center, 0.5, pitch);
            }
            return;
        }
        if edge {
            if name == "barrel" {
                let f = match self.prop(p, "facing").as_deref() {
                    Some("down") => [0.0, -0.5, 0.0],
                    Some("up") => [0.0, 0.5, 0.0],
                    Some("north") => [0.0, 0.0, -0.5],
                    Some("south") => [0.0, 0.0, 0.5],
                    Some("west") => [-0.5, 0.0, 0.0],
                    _ => [0.5, 0.0, 0.0],
                };
                let at = [center[0] + f[0], center[1] + f[1], center[2] + f[2]];
                let pitch = self.mobs.rng.next_float() * 0.1 + 0.9;
                self.world_sound(&format!("minecraft:block.barrel.{verb}"), 4, at, 0.5, pitch);
                if let Some(ns) = self.with_prop(st, "open", if open { "true" } else { "false" }) {
                    self.set_update(p, ns);
                }
            } else if name == "ender_chest" {
                let pitch = self.mobs.rng.next_float() * 0.1 + 0.9;
                self.world_sound(&format!("minecraft:block.ender_chest.{verb}"), 4, center, 0.5, pitch);
            } else {
                // Chest: sound at the center of the pair, played by the half that is not "left".
                let ty = self.prop(p, "type").unwrap_or_default();
                if ty != "left" {
                    let mut at = center;
                    if ty == "right"
                        && let Some((a, b)) = self.chest_pair(p)
                    {
                        let o = if a == p { b } else { a };
                        at[0] += f64::from(o[0] - p[0]) * 0.5;
                        at[2] += f64::from(o[2] - p[2]) * 0.5;
                    }
                    let set = if !name.contains("copper") {
                        "chest"
                    } else if name.contains("weathered") {
                        "copper_chest_weathered"
                    } else if name.contains("oxidized") {
                        "copper_chest_oxidized"
                    } else {
                        "copper_chest"
                    };
                    let pitch = self.mobs.rng.next_float() * 0.1 + 0.9;
                    self.world_sound(&format!("minecraft:block.{set}.{verb}"), 4, at, 0.5, pitch);
                }
            }
            self.game_event_block(event, p, Some(by), None);
        }
        if name.ends_with("chest") {
            self.block_event(p, block, 1, cur);
            if name == "trapped_chest" {
                // Trapped chest signal: neighbours warned.
                self.update_neighbors_at(p, block);
                self.update_neighbors_at([p[0], p[1] - 1, p[2]], block);
            }
        }
    }
}

/// Containers with a lock.
fn lockable(short: &str) -> bool {
    matches!(
        short,
        "chest"
            | "trapped_chest"
            | "barrel"
            | "dispenser"
            | "dropper"
            | "hopper"
            | "furnace"
            | "blast_furnace"
            | "smoker"
            | "brewing_stand"
            | "crafter"
            | "beacon"
    ) || short.ends_with("shulker_box")
        || short.ends_with("copper_chest")
}

/// Default name of a container, translation key.
fn default_title(short: &str) -> &'static str {
    match short {
        "barrel" => "container.barrel",
        "dispenser" => "container.dispenser",
        "dropper" => "container.dropper",
        "hopper" => "container.hopper",
        "furnace" => "container.furnace",
        "blast_furnace" => "container.blast_furnace",
        "smoker" => "container.smoker",
        "brewing_stand" => "container.brewing",
        "crafter" => "container.crafter",
        "beacon" => "container.beacon",
        s if s.ends_with("shulker_box") => "container.shulkerBox",
        _ => "container.chest",
    }
}

/// Total cooking time.
pub fn total_time(cooking: i32, speed: f32) -> i32 {
    if speed > 0.0 { (cooking as f32 / speed).ceil() as i32 } else { cooking }
}
