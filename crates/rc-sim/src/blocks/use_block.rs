//! Player right click on a block in singleplayer, block side: item on the block then
//! empty hand — cake (candle, eaten slice), candles put out by hand, flower
//! pot, composter, cauldrons (buckets, bottles, washing), TNT with flint and steel, bell, dragon
//! egg, jukebox, lectern, campfire, shelves, chiseled bookshelf, decorated pot, copper golem
//! statue, light block, head placed on a note block; water bottle on
//! dirt (mud).

use super::{Dir, Kind, P, below, flag};
use crate::mobs::mob::Ent;
use crate::player::Stack;
use crate::server::Server;

/// Response of a block to the held item (reduced).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ItemOn {
    /// Action consumed: nothing else.
    Done,
    /// The block lets the empty hand try (interaction result "try with empty hand").
    TryEmptyHand,
    /// Neither block nor empty hand: the item is used alone (interaction result "pass").
    Pass,
}

fn stack(id: &str) -> Stack {
    Stack { id: id.to_owned(), count: 1, components: None }
}

impl Server {
    /// Use of an item on a block among those handled here (main hand).
    pub(crate) fn block_use_item_on(&mut self, p: P, s: u32, face: Dir, at: [f32; 3]) -> ItemOn {
        let slot = 36 + self.held as usize;
        let held = self.slots[slot].clone();
        let id = held.as_ref().map_or("", |h| h.id.as_str()).to_owned();
        // G6-F01: blocks with their own block entity (family of the table, without comparing names).
        match self.v().kind(s) {
            Kind::Jukebox => return self.jukebox_use_item(p, s, slot),
            Kind::Lectern => return self.lectern_use_item(p, s, slot),
            Kind::Campfire => return self.campfire_use_item(p, s, slot),
            Kind::Shelf => return self.shelf_use_item(p, s, face, at, slot),
            Kind::ChiseledBookshelf => return self.bookshelf_use_item(p, s, face, at, slot),
            Kind::DecoratedPot => return self.pot_insert(p, s, slot),
            // Instrument head placed on top: the placement goes through.
            Kind::NoteBlock
                if face == Dir::Up && self.life.items.has_tag("minecraft:noteblock_top_instruments", &id) =>
            {
                return ItemOn::Pass;
            }
            _ => {}
        }
        let name = self.state_name(s).to_owned();
        let short = name.trim_start_matches("minecraft:");
        match short {
            "tnt" if id == "minecraft:flint_and_steel" || id == "minecraft:fire_charge" => {
                if self.tnt_prime_block_with(p, Some(Ent::Player), held.as_ref()) {
                    self.set_block(p, self.air(), 11);
                    if id == "minecraft:flint_and_steel" {
                        self.hurt_item(slot, 1);
                    } else {
                        self.consume_held(slot);
                    }
                    self.award_stat("minecraft:used", &id, 1);
                } else if self.rule("minecraft:tnt_explodes", true) {
                    // Adventure mode refusal: nothing happens, but the action counts as successful.
                } else {
                    let msg = rc_text::Component::translate("block.minecraft.tnt.disabled", Vec::new());
                    let _ = self.send(rc_client::Event::ActionBar(msg));
                    return ItemOn::Pass;
                }
                ItemOn::Done
            }
            "cake" => {
                if self.life.items.has_tag("minecraft:candles", &id) && self.v().get_int(s, "bites") == 0 {
                    let cake = format!("minecraft:{}_cake", id.trim_start_matches("minecraft:"));
                    let ns = self.default_state(&cake);
                    self.consume_held(slot);
                    self.block_sound(p, "minecraft:block.cake.add_candle", 1.0, 1.0);
                    self.set_update(p, ns);
                    self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
                    self.award_stat("minecraft:used", &id, 1);
                    ItemOn::Done
                } else {
                    ItemOn::TryEmptyHand
                }
            }
            _ if short.ends_with("candle_cake") => {
                if id == "minecraft:flint_and_steel" || id == "minecraft:fire_charge" {
                    ItemOn::Pass
                } else if at[1] > 0.5 && held.is_none() && self.v().get_bool(s, "lit") {
                    self.candle_extinguish(p, s);
                    ItemOn::Done
                } else {
                    ItemOn::TryEmptyHand
                }
            }
            _ if short.ends_with("candle") => {
                if held.is_none() && self.game_mode != 2 && self.v().get_bool(s, "lit") {
                    self.candle_extinguish(p, s);
                    ItemOn::Done
                } else {
                    ItemOn::TryEmptyHand
                }
            }
            "flower_pot" => self.pot_use_item(p, &id, slot, true),
            _ if short.starts_with("potted_") => self.pot_use_item(p, &id, slot, false),
            "composter" => self.composter_use_item(p, s, &id, slot),
            // W9: key inserted into an active vault.
            "vault" => match self.vault_use_item(p, slot) {
                Some(()) => ItemOn::Done,
                None => ItemOn::TryEmptyHand,
            },
            "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => match held {
                Some(h) => self.cauldron_use(p, s, short, &h, slot),
                None => ItemOn::TryEmptyHand,
            },
            _ if short.ends_with("copper_golem_statue") => self.statue_use_item(p, s, short, &id, slot),
            _ => ItemOn::TryEmptyHand,
        }
    }

    /// Use without an item of the blocks handled here; true if the action is consumed.
    pub(crate) fn block_use_without_item(&mut self, p: P, s: u32, face: Dir, at: [f32; 3]) -> bool {
        match self.v().kind(s) {
            Kind::Jukebox => return self.jukebox_use_without_item(p, s),
            Kind::Lectern => return self.lectern_use_without_item(p, s),
            Kind::ChiseledBookshelf => return self.bookshelf_use_without_item(p, s, face, at),
            Kind::DecoratedPot => return self.pot_fail(p),
            _ => {}
        }
        let name = self.state_name(s).to_owned();
        let short = name.trim_start_matches("minecraft:");
        match short {
            "cake" => self.cake_eat(p, s),
            _ if short.ends_with("candle_cake") => {
                let cake = self.default_state("minecraft:cake");
                if self.cake_eat(p, cake) {
                    // The candle drops (candle cake loot).
                    self.drop_resources(p, s, None);
                    true
                } else {
                    false
                }
            }
            _ if short.starts_with("potted_") => {
                let plant = match short {
                    "potted_azalea_bush" => "minecraft:azalea".to_owned(),
                    "potted_flowering_azalea_bush" => "minecraft:flowering_azalea".to_owned(),
                    _ => format!("minecraft:{}", &short["potted_".len()..]),
                };
                let selected = self.held as usize;
                let creative = self.game_mode == 1;
                if let Some(rest) = self.pickup_into_inventory(stack(&plant), creative, selected) {
                    self.drop_from_player(rest, false);
                }
                let pot = self.default_state("minecraft:flower_pot");
                self.set_update(p, pot);
                self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
                self.broadcast_menus();
                true
            }
            "flower_pot" => true,
            "composter" => {
                if self.v().get_int(s, "level") != 8 {
                    return false;
                }
                self.pop_resource_up(p, stack("minecraft:bone_meal"));
                let ns = self.v().set_int(s, "level", 0);
                self.set_update(p, ns);
                self.game_event_block("minecraft:block_change", p, Some(Ent::Player), Some(ns));
                self.block_sound(p, "minecraft:block.composter.empty", 1.0, 1.0);
                true
            }
            "bell" => self.bell_use(p, s, face, at),
            "dragon_egg" => {
                self.dragon_egg_teleport(p, s);
                true
            }
            // Light block: next level for a creative player who has the commands.
            "light" => {
                if self.gamemaster_blocks() {
                    let ns = self.v().cycle(s, "level");
                    self.set_block(p, ns, flag::CLIENTS);
                }
                true
            }
            _ => false,
        }
    }

    /// Decorated pot, held item: empty pot or same non-full item → one unit
    /// deposited (wobble, statistic, sound higher-pitched as it fills, dust).
    fn pot_insert(&mut self, p: P, s: u32, slot: usize) -> ItemOn {
        // Contents consulted: still-sealed loot drawn first (without a player, like the game).
        self.unpack_loot(p, false);
        let Some(be) = self.be_get(p) else { return ItemOn::Pass };
        let inside = be.get("item").and_then(Stack::from_nbt);
        let Some(held) = self.slots[slot].clone() else { return ItemOn::TryEmptyHand };
        let fits = match &inside {
            None => true,
            Some(i) => crate::items::same(i, &held) && i.count < self.life.items.max_stack(i),
        };
        if !fits {
            return ItemOn::TryEmptyHand;
        }
        let b = self.v().block(s);
        self.block_event(p, b, 1, 0);
        self.award_stat("minecraft:used", &held.id, 1);
        self.consume_held(slot);
        let now = match inside {
            None => Stack { count: 1, ..held },
            Some(i) => Stack { count: i.count + 1, ..i },
        };
        let bend = now.count as f32 / self.life.items.max_stack(&now) as f32;
        if let Some(be) = self.be_get_mut(p) {
            crate::nbtio::set(be, "item", now.to_nbt(None));
        }
        self.block_sound(p, "minecraft:block.decorated_pot.insert", 1.0, 0.7 + 0.5 * bend);
        let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 1.2, f64::from(p[2]) + 0.5];
        self.particles("minecraft:dust_plume", at, [0.0; 3], 0.0, 7);
        self.update_neighbour_for_output_signal(p, b);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
        self.broadcast_menus();
        ItemOn::Done
    }

    /// Decorated pot, empty hand: failure sound, reverse wobble, event.
    fn pot_fail(&mut self, p: P) -> bool {
        if self.be_get(p).is_none() {
            return false;
        }
        self.block_sound(p, "minecraft:block.decorated_pot.insert_fail", 1.0, 1.0);
        let b = self.v().block(self.get(p));
        self.block_event(p, b, 1, 1);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
        true
    }

    /// Block entity of the pot removed (from a container): item scattered.
    pub(crate) fn pot_pre_remove(&mut self, p: P) {
        self.unpack_loot(p, false); // sealed loot drawn before being scattered
        let Some(item) = self.be_get(p).and_then(|b| b.get("item")).and_then(Stack::from_nbt) else { return };
        self.drop_contents([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])], item);
    }

    /// Copper golem statue: any item except an axe (and wax on a
    /// statue that oxidizes) changes the pose; axe on an intact unwaxed statue → golem.
    fn statue_use_item(&mut self, p: P, s: u32, short: &str, id: &str, slot: usize) -> ItemOn {
        let axe = self.life.items.has_tag("minecraft:axes", id);
        let waxed = short.starts_with("waxed_");
        if waxed {
            if axe {
                return ItemOn::Pass;
            }
        } else {
            if self.be_get(p).is_none() {
                return ItemOn::Pass;
            }
            if !axe && id == "minecraft:honeycomb" {
                return ItemOn::Pass;
            }
            if axe {
                if short == "copper_golem_statue" {
                    let golem = self.statue_golem(p, s);
                    self.hurt_item(slot, 1);
                    self.add_mob(golem);
                    self.remove_block(p, false);
                    return ItemOn::Done;
                }
                return ItemOn::Pass;
            }
        }
        // Sound, next pose, event attributed to the player.
        self.block_sound(p, "minecraft:entity.copper_golem_become_statue", 1.0, 1.0);
        let ns = self.v().cycle(s, "copper_golem_pose");
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
        ItemOn::Done
    }

    /// Water bottle on a `convertible_to_mud` block (not from below): mud, sounds, drops,
    /// empty bottle given back. True if the action took place.
    pub(crate) fn water_bottle_on_mud(&mut self, slot: usize, p: P, face: Dir) -> bool {
        let Some(h) = self.slots[slot].clone() else { return false };
        if face == Dir::Down || !Self::is_water_potion(&h) {
            return false;
        }
        let b = u32::from(self.v().block(self.get(p)));
        if !self.block_tags.get("minecraft:convertible_to_mud").is_some_and(|t| t.binary_search(&b).is_ok()) {
            return false;
        }
        self.block_sound(p, "minecraft:entity.generic.splash", 1.0, 1.0);
        self.filled_result(slot, stack("minecraft:glass_bottle"), true);
        for _ in 0..5 {
            let x = f64::from(p[0]) + self.life.rng.next_double();
            let z = f64::from(p[2]) + self.life.rng.next_double();
            self.particles("minecraft:splash", [x, f64::from(p[1] + 1), z], [0.0; 3], 1.0, 1);
        }
        self.block_sound(p, "minecraft:item.bottle.empty", 1.0, 1.0);
        self.game_event_block("minecraft:fluid_place", p, None, None);
        let mud = self.default_state("minecraft:mud");
        self.set_update(p, mud);
        true
    }

    /// Candle (or candle cake) put out: state, sound, `block_change` event.
    fn candle_extinguish(&mut self, p: P, s: u32) {
        let ns = self.v().set_bool(s, "lit", false);
        self.set_block(p, ns, 11);
        self.block_sound(p, "minecraft:block.candle.extinguish", 1.0, 1.0);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
    }

    /// Only if the player can eat; slice (2, saturation 0.4), one
    /// more bite, finished cake removed.
    fn cake_eat(&mut self, p: P, s: u32) -> bool {
        if !(self.abilities().invulnerable || self.life.vit.food < 20) {
            return false;
        }
        self.award_stat("minecraft:custom", "minecraft:eat_cake_slice", 1);
        let v = &mut self.life.vit;
        v.food = (v.food + 2).clamp(0, 20);
        v.saturation = (v.saturation + 2.0 * 0.1 * 2.0).clamp(0.0, v.food as f32);
        self.game_event_block("minecraft:eat", p, Some(Ent::Player), None);
        let bites = self.v().get_int(s, "bites");
        if bites < 6 {
            let ns = self.v().set_int(s, "bites", bites + 1);
            self.set_update(p, ns);
        } else {
            self.remove_block(p, false);
            self.game_event_block("minecraft:block_destroy", p, Some(Ent::Player), None);
        }
        true
    }

    /// Flower pot: plant placed in an empty pot (statistic, item consumed); a full
    /// pot keeps the action without doing anything.
    fn pot_use_item(&mut self, p: P, item: &str, slot: usize, empty: bool) -> ItemOn {
        let short = item.trim_start_matches("minecraft:");
        let potted = match short {
            "azalea" => "minecraft:potted_azalea_bush".to_owned(),
            "flowering_azalea" => "minecraft:potted_flowering_azalea_bush".to_owned(),
            "" => return ItemOn::TryEmptyHand,
            _ => format!("minecraft:potted_{short}"),
        };
        if self.tb().block_id(&potted) == u16::MAX {
            return ItemOn::TryEmptyHand;
        }
        if !empty {
            return ItemOn::Done;
        }
        let ns = self.default_state(&potted);
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Player), None);
        self.award_stat("minecraft:custom", "minecraft:pot_flower", 1);
        self.consume_held(slot);
        ItemOn::Done
    }

    /// Composter: compostable item below level 8; layer drawn below 7 (event 1500,
    /// 1 if the level rises), item consumed.
    fn composter_use_item(&mut self, p: P, s: u32, item: &str, slot: usize) -> ItemOn {
        let level = self.v().get_int(s, "level");
        let compostable = !item.is_empty() && !self.life.items.defaults(item)["minecraft:compostable"].is_null();
        if level >= 8 || !compostable {
            return ItemOn::TryEmptyHand;
        }
        if level < 7 {
            let ns = self.composter_add_layer(p, s, item, Some(Ent::Player));
            self.level_event(1500, p, i32::from(ns != s));
            self.award_stat("minecraft:used", item, 1);
            self.consume_held(slot);
        }
        ItemOn::Done
    }

    /// Layers drawn for the item; if any, level capped at 7, placement,
    /// `block_change` event (author `by`), switch to 8 scheduled at level 7. Returns the state.
    pub(crate) fn composter_add_layer(&mut self, p: P, s: u32, item: &str, by: Option<Ent>) -> u32 {
        let level = self.v().get_int(s, "level");
        let layers = self.compost_layers(item, level);
        if layers <= 0 {
            return s;
        }
        let ns = self.v().set_int(s, "level", (level + layers).clamp(0, 7));
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, by, Some(ns));
        if self.v().get_int(ns, "level") == 7 {
            self.schedule_block_tick(p, 20);
        }
        ns
    }

    /// Item given back after a filling: creative, the item is added
    /// if missing (always if `limit` is false) and the stack stays; otherwise one unit goes and
    /// the item replaces it or joins the inventory.
    pub(crate) fn filled_result(&mut self, slot: usize, new: Stack, limit: bool) {
        let selected = self.held as usize;
        if self.game_mode == 1 {
            let has = self.slots.iter().flatten().any(|t| t.id == new.id && t.components == new.components);
            if !limit || !has {
                let _ = self.pickup_into_inventory(new, true, selected);
            }
            return;
        }
        let Some(mut cur) = self.slots[slot].clone() else { return };
        cur.count -= 1;
        if cur.count <= 0 {
            self.slots[slot] = Some(new);
        } else {
            self.slots[slot] = Some(cur);
            if let Some(rest) = self.pickup_into_inventory(new, false, selected) {
                self.drop_from_player(rest, false);
            }
        }
        self.broadcast_menus();
    }

    fn is_water_potion(s: &Stack) -> bool {
        use rc_proto::nbt::Nbt;
        if s.id != "minecraft:potion" {
            return false;
        }
        match crate::items::component(s, "minecraft:potion_contents") {
            Some(Nbt::String(v)) => v == "minecraft:water",
            Some(c @ Nbt::Compound(_)) => c.get("potion").and_then(Nbt::as_str) == Some("minecraft:water"),
            _ => false,
        }
    }

    /// Cauldrons: buckets (water, lava, powder snow) emptied or filled, water bottles, washing
    /// of dyes, banners and Shulker boxes.
    fn cauldron_use(&mut self, p: P, s: u32, kind: &str, h: &Stack, slot: usize) -> ItemOn {
        let id = h.id.as_str();
        let level =
            if kind == "water_cauldron" || kind == "powder_snow_cauldron" { self.v().get_int(s, "level") } else { 0 };
        let under_water = self.fluid_at([p[0], p[1] + 1, p[2]]).kind == 1;
        // Full buckets: valid on all cauldrons.
        let empty_bucket = |to: &str, lvl: Option<i32>, sound: &str| (to.to_owned(), lvl, sound.to_owned());
        let pour = match id {
            "minecraft:water_bucket" => {
                Some(empty_bucket("minecraft:water_cauldron", Some(3), "minecraft:item.bucket.empty"))
            }
            "minecraft:lava_bucket" if !under_water => {
                Some(empty_bucket("minecraft:lava_cauldron", None, "minecraft:item.bucket.empty_lava"))
            }
            "minecraft:powder_snow_bucket" if !under_water => {
                Some(empty_bucket("minecraft:powder_snow_cauldron", Some(3), "minecraft:item.bucket.empty_powder_snow"))
            }
            "minecraft:lava_bucket" | "minecraft:powder_snow_bucket" => return ItemOn::Done,
            _ => None,
        };
        if let Some((to, lvl, sound)) = pour {
            let mut ns = self.default_state(&to);
            if let Some(l) = lvl {
                ns = self.v().set_int(ns, "level", l);
            }
            self.filled_result(slot, stack("minecraft:bucket"), true);
            self.award_stat("minecraft:custom", "minecraft:fill_cauldron", 1);
            self.award_stat("minecraft:used", id, 1);
            self.set_update(p, ns);
            self.block_sound(p, &sound, 1.0, 1.0);
            self.game_event_block("minecraft:fluid_place", p, None, None);
            return ItemOn::Done;
        }
        match (kind, id) {
            ("water_cauldron" | "lava_cauldron" | "powder_snow_cauldron", "minecraft:bucket") => {
                let (full, sound) = match kind {
                    "water_cauldron" => ("minecraft:water_bucket", "minecraft:item.bucket.fill"),
                    "lava_cauldron" => ("minecraft:lava_bucket", "minecraft:item.bucket.fill_lava"),
                    _ => ("minecraft:powder_snow_bucket", "minecraft:item.bucket.fill_powder_snow"),
                };
                if kind != "lava_cauldron" && level != 3 {
                    return ItemOn::TryEmptyHand;
                }
                self.filled_result(slot, stack(full), true);
                self.award_stat("minecraft:custom", "minecraft:use_cauldron", 1);
                self.award_stat("minecraft:used", id, 1);
                let empty = self.default_state("minecraft:cauldron");
                self.set_update(p, empty);
                self.block_sound(p, sound, 1.0, 1.0);
                self.game_event_block("minecraft:fluid_pickup", p, None, None);
                ItemOn::Done
            }
            ("water_cauldron", "minecraft:glass_bottle") => {
                let water = Stack {
                    id: "minecraft:potion".into(),
                    count: 1,
                    components: Some(rc_proto::nbt::Nbt::Compound(vec![(
                        "minecraft:potion_contents".into(),
                        rc_proto::nbt::Nbt::Compound(vec![(
                            "potion".into(),
                            rc_proto::nbt::Nbt::String("minecraft:water".into()),
                        )]),
                    )])),
                };
                self.filled_result(slot, water, true);
                self.award_stat("minecraft:custom", "minecraft:use_cauldron", 1);
                self.award_stat("minecraft:used", id, 1);
                self.cauldron_lower(p, s);
                self.block_sound(p, "minecraft:item.bottle.fill", 1.0, 1.0);
                self.game_event_block("minecraft:fluid_pickup", p, None, None);
                ItemOn::Done
            }
            ("cauldron" | "water_cauldron", "minecraft:potion") => {
                if kind == "water_cauldron" && level == 3 {
                    return ItemOn::TryEmptyHand;
                }
                if !Self::is_water_potion(h) {
                    return ItemOn::TryEmptyHand;
                }
                self.filled_result(slot, stack("minecraft:glass_bottle"), true);
                self.award_stat("minecraft:custom", "minecraft:use_cauldron", 1);
                self.award_stat("minecraft:used", id, 1);
                let ns = if kind == "cauldron" {
                    self.default_state("minecraft:water_cauldron")
                } else {
                    self.v().set_int(s, "level", level + 1)
                };
                self.set_update(p, ns);
                self.block_sound(p, "minecraft:item.bottle.empty", 1.0, 1.0);
                self.game_event_block("minecraft:fluid_place", p, None, None);
                ItemOn::Done
            }
            ("water_cauldron", _) => self.cauldron_wash(p, s, h, slot),
            _ => ItemOn::TryEmptyHand,
        }
    }

    /// Washing in water: dye removed (washable items), last banner pattern,
    /// dyed Shulker box made neutral; one water level less.
    fn cauldron_wash(&mut self, p: P, s: u32, h: &Stack, slot: usize) -> ItemOn {
        use rc_proto::nbt::Nbt;
        let id = h.id.as_str();
        if self.life.items.has_tag("minecraft:cauldron_can_remove_dye", id) {
            if crate::items::component(h, "minecraft:dyed_color").is_none() {
                return ItemOn::TryEmptyHand;
            }
            if let Some(cur) = self.slots[slot].as_mut()
                && let Some(Nbt::Compound(c)) = cur.components.as_mut()
            {
                c.retain(|(k, _)| k != "minecraft:dyed_color");
                if c.is_empty() {
                    cur.components = None;
                }
            }
            self.award_stat("minecraft:custom", "minecraft:clean_armor", 1);
            self.cauldron_lower(p, s);
            self.broadcast_menus();
            return ItemOn::Done;
        }
        if id.ends_with("_banner") {
            let Some(Nbt::List(layers)) = crate::items::component(h, "minecraft:banner_patterns").cloned() else {
                return ItemOn::TryEmptyHand;
            };
            if layers.is_empty() {
                return ItemOn::TryEmptyHand;
            }
            let mut clean = Stack { count: 1, ..h.clone() };
            if let Some(Nbt::Compound(c)) = clean.components.as_mut() {
                let mut l = layers.clone();
                l.pop();
                for e in c.iter_mut() {
                    if e.0 == "minecraft:banner_patterns" {
                        e.1 = Nbt::List(l.clone());
                    }
                }
            }
            self.filled_result(slot, clean, false);
            self.award_stat("minecraft:custom", "minecraft:clean_banner", 1);
            self.cauldron_lower(p, s);
            return ItemOn::Done;
        }
        if id.ends_with("_shulker_box") {
            let clean = Stack { id: "minecraft:shulker_box".into(), count: 1, components: h.components.clone() };
            self.filled_result(slot, clean, false);
            self.award_stat("minecraft:custom", "minecraft:clean_shulker_box", 1);
            self.cauldron_lower(p, s);
            return ItemOn::Done;
        }
        ItemOn::TryEmptyHand
    }

    /// One level less, cauldron empty at 0, `block_change` event.
    pub(crate) fn cauldron_lower(&mut self, p: P, s: u32) {
        let level = self.v().get_int(s, "level") - 1;
        let ns =
            if level == 0 { self.default_state("minecraft:cauldron") } else { self.v().set_int(s, "level", level) };
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, None, Some(ns));
    }

    /// Bell struck by hand: face on the right side according to the attachment (not the top nor the
    /// bottom, not above 0.8124), block event 1 (direction), sound, statistic.
    fn bell_use(&mut self, p: P, s: u32, face: Dir, at: [f32; 3]) -> bool {
        self.bell_on_hit(p, s, face, f64::from(at[1]), true)
    }

    /// Bell struck (hand or projectile) on face `face`, at height `rel_y` in the block:
    /// only from the right side according to the attachment (never top nor bottom, never above
    /// 0.8124); the ringing player gains the statistic. True if the strike counts.
    pub(crate) fn bell_on_hit(&mut self, p: P, s: u32, face: Dir, rel_y: f64, by_player: bool) -> bool {
        if face.axis() == 1 || rel_y > f64::from(0.8124f32) {
            return false;
        }
        let facing = self.v().facing(s);
        let proper = match self.v().get(s, "attachment").unwrap_or("floor") {
            "floor" => facing.axis() == face.axis(),
            "single_wall" | "double_wall" => facing.axis() != face.axis(),
            "ceiling" => true,
            _ => false,
        };
        if !proper {
            return false;
        }
        let by = by_player.then_some(Ent::Player);
        if self.bell_attempt_ring(p, by, Some(face)) && by_player {
            self.award_stat("minecraft:custom", "minecraft:bell_ring", 1);
        }
        true
    }

    /// Ringing of a bell (block entity required): block event 1 with the direction of the blow
    /// (that of the bell by default), sound, "block changed" game event of `by`.
    pub(crate) fn bell_attempt_ring(&mut self, p: P, by: Option<Ent>, dir: Option<Dir>) -> bool {
        if self.block_entity(p).is_none() {
            return false;
        }
        let s = self.get(p);
        let dir = dir.unwrap_or_else(|| self.v().facing(s));
        let b = self.v().block(s);
        self.block_event(p, b, 1, dir as i32);
        self.block_sound(p, "minecraft:block.bell.use", 2.0, 1.0);
        self.game_event_block("minecraft:block_change", p, by, None);
        true
    }

    /// Dragon egg (right click or strike): 1000 attempts at an air cell placed on a non-empty
    /// block, within ±15/±7/±15, the border and the height of the world; event 2015 (trail,
    /// difference compacted around 16/8/16), the egg is placed there then removed from its place.
    pub(crate) fn dragon_egg_teleport(&mut self, p: P, s: u32) {
        for _ in 0..1000 {
            let r = &mut self.bs.rng;
            let dx = r.next_int_bound(16) - r.next_int_bound(16);
            let dy = r.next_int_bound(8) - r.next_int_bound(8);
            let dz = r.next_int_bound(16) - r.next_int_bound(16);
            let t = [p[0] + dx, p[1] + dy, p[2] + dz];
            if self.v().is_air(self.get(t))
                && !self.v().is_air(self.get(below(t)))
                && Self::block_in_border(self.border_bounds(), t)
                && self.in_build_height(t[1])
            {
                let packed = (((dx + 16) & 255) << 16) | (((dy + 8) & 255) << 8) | ((dz + 16) & 255);
                self.level_event(2015, p, packed);
                self.set_block(t, s, flag::CLIENTS);
                self.remove_block(p, false);
                return;
            }
        }
    }
}
