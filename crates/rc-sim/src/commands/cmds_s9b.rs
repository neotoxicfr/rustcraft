//! Commands rewritten or completed by S9 (`commands.md` § 6.4): `loot` (all sources
//! and targets), `item` (replace, fill, override, modify; slot ranges), `attribute`
//! (modifiers, computed value, base reset).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use serde_json::Value;

use super::cmds_basic::Track;
use super::cmds_data::{equipment_index, slot_ids};
use super::ents::E;
use super::src::{Fail, Src};
use super::text;
use super::{Parsed, R, Seg, args};
use crate::player::Stack;
use crate::progress::pred::{Ctx as PCtx, Dmg, Who};
use crate::server::Server;
use crate::tr;

/// NBT (SNBT of a command) → codec JSON (bytes 0/1 read as booleans).
pub fn nbt_to_json(n: &Nbt) -> Value {
    match n {
        Nbt::Byte(b) if *b == 0 || *b == 1 => Value::Bool(*b == 1),
        Nbt::Byte(b) => Value::from(*b),
        Nbt::Short(v) => Value::from(*v),
        Nbt::Int(v) => Value::from(*v),
        Nbt::Long(v) => Value::from(*v),
        Nbt::Float(v) => serde_json::Number::from_f64(f64::from(*v)).map_or(Value::Null, Value::Number),
        Nbt::Double(v) => serde_json::Number::from_f64(*v).map_or(Value::Null, Value::Number),
        Nbt::String(s) => Value::String(s.clone()),
        Nbt::List(l) => Value::Array(l.iter().map(nbt_to_json).collect()),
        Nbt::ByteArray(a) => Value::Array(a.iter().map(|x| Value::from(*x)).collect()),
        Nbt::IntArray(a) => Value::Array(a.iter().map(|x| Value::from(*x)).collect()),
        Nbt::LongArray(a) => Value::Array(a.iter().map(|x| Value::from(*x)).collect()),
        Nbt::Compound(f) => Value::Object(f.iter().map(|(k, v)| (k.clone(), nbt_to_json(v))).collect()),
        #[allow(unreachable_patterns)]
        _ => Value::Null,
    }
}

/// Slot designated by a range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slot {
    /// Index in the player's menu 0.
    Player(usize),
    /// A player's equipment slot (held hand, other hand, armor) reached through the equipment,
    /// index in the player's menu 0: a write equips the stack, with its sound and game event.
    PlayerEquipment(usize),
    Ender(usize),
    /// Equipment of a mob (network order: main hand, off hand, feet, legs, chest, head, body, saddle).
    Mob(i32, usize),
    /// Slot of a container.
    Block([i32; 3], usize),
    /// Item on the ground (its stack).
    Drop(i32),
    /// Stack stored in the NBT of another entity: field, then optional subfield (frame:
    /// `Item`; armor stand: `equipment.<slot>`).
    Nbt(E, &'static str, Option<&'static str>),
}

/// Target of an item command.
#[derive(Debug, Clone)]
pub enum Holder {
    Ents(Vec<E>),
    Block([i32; 3]),
}

/// One container's slots decoded once for the reads of a command (`RUSTCRAFT_OPT_SLOTVIEW`),
/// where `container_item` decodes them again at every read. A write by the command reads its
/// slot again from the container (the container's own write rules decide what it holds).
#[derive(Default)]
pub(crate) struct SlotView {
    at: Option<[i32; 3]>,
    items: Vec<Option<Stack>>,
}

/// Equipment field of an armor stand for each equipment slot (order of
/// `equipment_index`: main hand, other hand, feet, legs, chest, head, body, saddle).
const EQUIP_KEYS: [&str; 8] = ["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"];

impl Server {
    /// Slots of an entity for a slot name: identifiers from the game's slot table
    /// (`slot_ids`), then what each kind of entity stores under each identifier. A slot that
    /// the entity does not have is omitted.
    pub(crate) fn entity_slot_range(&self, e: E, name: &str) -> Vec<Slot> {
        self.entity_slots(e, slot_ids(name))
    }

    /// Slots of an entity for slot identifiers of the game's table (one that the entity does
    /// not have is omitted).
    pub(crate) fn entity_slots(&self, e: E, ids: impl IntoIterator<Item = i32>) -> Vec<Slot> {
        let ids = ids.into_iter();
        match e {
            E::Player => ids.filter_map(|id| self.player_slot(id)).collect(),
            E::Mob(m) => ids.filter_map(equipment_index).map(|k| Slot::Mob(m, k)).collect(),
            E::Item(i) => ids.filter(|&id| id == 0).map(|_| Slot::Drop(i)).collect(),
            E::Other(_) => match self.ent_type(e).as_str() {
                "minecraft:item_frame" | "minecraft:glow_item_frame" => {
                    ids.filter(|&id| id == 0).map(|_| Slot::Nbt(e, "Item", None)).collect()
                }
                // Living entities in the game: same equipment slots as a mob.
                "minecraft:armor_stand" | "minecraft:mannequin" => {
                    ids.filter_map(equipment_index).map(|k| Slot::Nbt(e, "equipment", Some(EQUIP_KEYS[k]))).collect()
                }
                _ => Vec::new(),
            },
            E::Orb(_) => Vec::new(),
        }
    }

    /// Player slot for an identifier: inventory 0-35, held hand 98, other hand 99,
    /// armor 100-103 (feet to head), Ender chest 200-226, crafting 500-503 (also reached
    /// by `horse.0`-`horse.3`, which share these identifiers in the game).
    fn player_slot(&self, id: i32) -> Option<Slot> {
        Some(match id {
            0..=35 => Slot::Player(crate::progress::pred::inv_to_menu(id as usize)),
            98 => Slot::PlayerEquipment(36 + self.held as usize),
            99 => Slot::PlayerEquipment(45),
            100..=103 => Slot::PlayerEquipment((108 - id) as usize),
            200..=226 => Slot::Ender((id - 200) as usize),
            500..=503 => Slot::Player((id - 499) as usize),
            _ => return None,
        })
    }

    /// Slots of a container: identifiers of the slot table below its size.
    pub(crate) fn block_slot_range(&self, p: [i32; 3], name: &str) -> Vec<Slot> {
        let size = self.command_container_size(p);
        slot_ids(name)
            .into_iter()
            .filter(|&i| i >= 0 && (i as usize) < size)
            .map(|i| Slot::Block(p, i as usize))
            .collect()
    }

    fn command_container_size(&self, p: [i32; 3]) -> usize {
        container_size(self.be_get(p).and_then(|b| b.get("id")).and_then(Nbt::as_str).unwrap_or(""))
    }

    /// Block entity that is an item container for the commands.
    fn is_container(&self, p: [i32; 3]) -> bool {
        self.be_get(p).is_some_and(|be| is_container_id(be.get("id").and_then(Nbt::as_str).unwrap_or("")))
    }

    /// The game's placement test of slot `i` of the container at `p` for the commands (no
    /// face): that of the hoppers' containers (furnace, brewing stand, jukebox, crafter), Shulker
    /// boxes accepting boxes outside a face; the chiseled bookshelf takes one book per empty
    /// slot, the shelf a stack below its size.
    fn container_accepts(&self, p: [i32; 3], i: usize, item: &Stack) -> bool {
        let id = self.be_get(p).and_then(|b| b.get("id")).and_then(Nbt::as_str).unwrap_or("");
        let cur = || self.slot_get(Slot::Block(p, i));
        match id.trim_start_matches("minecraft:") {
            "chiseled_bookshelf" => self.life.items.has_tag("minecraft:bookshelf_books", &item.id) && cur().is_none(),
            "shelf" => cur().is_none_or(|c| c.count < self.life.items.max_stack(item).min(99)),
            _ => self.command_can_place(p, i, item),
        }
    }

    /// Slot read by a command (`item`, `execute if items`, `loot insert`): like the game's
    /// container read, a sealed loot container draws its loot first.
    pub(crate) fn command_slot_get(&mut self, s: Slot) -> Option<Stack> {
        if let Slot::Block(p, _) = s {
            self.unseal_for_access(p);
        }
        self.slot_get(s)
    }

    /// `command_slot_get` through `view`: a container is decoded at its first read (which draws
    /// its sealed loot, like every read) and read from the copy afterwards.
    pub(crate) fn command_slot_read(&mut self, s: Slot, view: &mut SlotView) -> Option<Stack> {
        match s {
            Slot::Block(p, i) if self.cmd.paths.slot_view => {
                if view.at != Some(p) {
                    self.unseal_for_access(p);
                    view.items = self.container_slots_as_read(p, self.command_container_size(p));
                    view.at = Some(p);
                }
                view.items.get(i).cloned().flatten()
            }
            _ => self.command_slot_get(s),
        }
    }

    /// `slot_set` keeping `view` right: the written slot is read again from the container.
    pub(crate) fn command_slot_write(&mut self, s: Slot, v: Option<Stack>, view: &mut SlotView) -> bool {
        let written = self.slot_set(s, v);
        self.slot_view_reread(s, view);
        written
    }

    /// A slot of `view`'s container changed by the command: read again.
    fn slot_view_reread(&self, s: Slot, view: &mut SlotView) {
        if let Slot::Block(p, i) = s
            && view.at == Some(p)
            && let Some(slot) = view.items.get_mut(i)
        {
            *slot = self.container_item(p, i, self.command_container_size(p));
        }
    }

    pub(crate) fn slot_get(&self, s: Slot) -> Option<Stack> {
        match s {
            Slot::Player(i) | Slot::PlayerEquipment(i) => self.slots.get(i).cloned().flatten(),
            Slot::Ender(i) => self.life.vit.ender.get(i).cloned().flatten(),
            Slot::Mob(id, i) => self.mobs.mob_ref_any(id).and_then(|m| m.equipment[i].clone()),
            Slot::Block(p, i) => self.container_item(p, i, self.command_container_size(p)),
            Slot::Drop(i) => self.life.drops.iter().find(|d| d.id == i).map(|d| d.stack.clone()),
            Slot::Nbt(e, k, sub) => {
                let n = self.ent_nbt(e);
                let v = n.get(k)?;
                Stack::from_nbt(match sub {
                    Some(s) => v.get(s)?,
                    None => v,
                })
            }
        }
    }

    pub(crate) fn slot_set(&mut self, s: Slot, v: Option<Stack>) -> bool {
        let v = v.filter(|x| x.count > 0 && x.id != "minecraft:air");
        match s {
            // Player armor (menu 6 chest, 7 legs, 8 feet): same equipment rule as the mobs.
            Slot::PlayerEquipment(i @ 6..=8) if !self.equipment_accepts(E::Player, 10 - i, v.as_ref()) => false,
            Slot::PlayerEquipment(i) if i < self.slots.len() => {
                let old = std::mem::replace(&mut self.slots[i], v.clone());
                // Written through the equipment, like the game: the equip sound and game event,
                // in the player's own world even for a command run in another dimension.
                self.in_players_world(|s| s.on_equip_item(i, old.as_ref(), v.as_ref()));
                self.trig_inventory_scan();
                true
            }
            Slot::PlayerEquipment(_) => false,
            Slot::Player(i) => {
                if i < self.slots.len() {
                    self.slots[i] = v;
                    // The game broadcasts the menu at once: `inventory_changed` before the message.
                    self.trig_inventory_scan();
                    true
                } else {
                    false
                }
            }
            Slot::Ender(i) => {
                if i < self.life.vit.ender.len() {
                    self.life.vit.ender[i] = v;
                    true
                } else {
                    false
                }
            }
            Slot::Mob(id, i) if !self.equipment_accepts(E::Mob(id), i, v.as_ref()) => false,
            Slot::Nbt(e, "equipment", Some(k))
                if !self.equipment_accepts(e, EQUIP_KEYS.iter().position(|x| *x == k).unwrap_or(0), v.as_ref()) =>
            {
                false
            }
            // Written through the living entity's equipment, like the game: its equip sound and
            // game event.
            Slot::Mob(id, i) => {
                let Some(at) = self.mobs.index_of(id) else { return false };
                let Some(mut m) = self.mobs.list.take(at) else { return false };
                self.mob_set_item_slot(&mut m, i, v);
                m.needs_sync = true;
                self.mobs.list.put(at, m);
                true
            }
            // The container's own slot write (not a reload of its whole data).
            Slot::Block(p, i) => {
                let size = self.command_container_size(p);
                if self.be_get(p).is_none() || i >= size {
                    return false;
                }
                self.container_set_item(p, i, size, v);
                true
            }
            // Emptied stack: the item on the ground keeps its own (the game removes it at the next tick;
            // not reproduced).
            Slot::Drop(i) => match (self.life.drops.iter_mut().find(|d| d.id == i), v) {
                (Some(d), Some(st)) => {
                    d.stack = st;
                    true
                }
                _ => false,
            },
            // An armor stand's or a mannequin's equipment: the living entity's reaction too.
            Slot::Nbt(e, "equipment", Some(slot)) => {
                let old = self.slot_get(s);
                self.write_entity_compound_slot(e, "equipment", Some(slot), v.as_ref());
                let n = self.ent_nbt(e);
                self.living_decoration_equip_reaction(self.ent_id(e), &n, slot, old.as_ref(), v.as_ref());
                true
            }
            Slot::Nbt(e, k, sub) => {
                self.write_entity_compound_slot(e, k, sub, v.as_ref());
                true
            }
        }
    }

    /// Writes stack `v` (or removes it) at field `k` (or `k.sub`) of entity `e`'s compound.
    fn write_entity_compound_slot(&mut self, e: E, k: &str, sub: Option<&str>, v: Option<&Stack>) {
        let mut n = self.ent_nbt(e);
        let put = |c: &mut Nbt, key: &str| match v {
            Some(st) => crate::nbtio::set(c, key, st.to_nbt(None)),
            None => crate::nbtio::remove(c, key),
        };
        match sub {
            Some(s) => {
                let mut c = n.get(k).cloned().unwrap_or(Nbt::Compound(Vec::new()));
                put(&mut c, s);
                crate::nbtio::set(&mut n, k, c);
            }
            None => put(&mut n, k),
        }
        self.ent_set_nbt(e, &n);
    }

    /// Equipment slot (network order) where a living entity puts an item: that of its
    /// `equippable` component if the entity can use that slot, otherwise the main hand.
    fn entity_equipment_slot_for(&self, e: E, st: &Stack) -> usize {
        // A mob: the common rule, with each species' own slots (saddle, body armor of the young).
        if let E::Mob(i) = e
            && let Some(m) = self.mobs.mob_ref_any(i)
        {
            return self.equipment_slot_for(m, st);
        }
        let k = self.slot_named_by_item(st).unwrap_or(0);
        // Armor stand: neither body nor saddle, nor a disabled slot. Mannequins: every slot.
        let usable = match e {
            E::Other(_) if self.ent_type(e) == "minecraft:armor_stand" => {
                k < 6 && !crate::decoration::stand::is_disabled(&self.ent_nbt(e), k)
            }
            _ => true,
        };
        if usable { k } else { 0 }
    }

    /// The game's slot access of a living entity's equipment: the hands and the head take any
    /// item; the other slots only an item that goes there (or nothing).
    fn equipment_accepts(&self, e: E, k: usize, v: Option<&Stack>) -> bool {
        match v {
            Some(st) if matches!(k, 2 | 3 | 4 | 6 | 7) => self.entity_equipment_slot_for(e, st) == k,
            _ => true,
        }
    }

    /// Item modifier (registry identifier or inline content) → JSON functions.
    fn arg_item_modifier(&self, text: &str) -> Result<Value, Fail> {
        let t = text.trim();
        if t.starts_with('{') || t.starts_with('[') {
            let n = super::snbt::parse(t).map_err(|_| Fail(tr!("argument.nbt.expected.compound")))?;
            return Ok(nbt_to_json(&n));
        }
        let id = args::id(t);
        self.life
            .pack
            .get("item_modifier", &id)
            .map(|v| (*v).clone())
            .ok_or_else(|| Fail(tr!("argument.resource_or_id.failed_to_parse", id)))
    }

    /// Loot table (identifier or inline content); a table of the pack keeps its shared data
    /// (its identity when it nests itself).
    fn arg_loot_table(&self, text: &str) -> Result<(Option<String>, Arc<Value>), Fail> {
        let t = text.trim();
        if t.starts_with('{') {
            let n = super::snbt::parse(t).map_err(|_| Fail(tr!("argument.nbt.expected.compound")))?;
            return Ok((None, Arc::new(nbt_to_json(&n))));
        }
        let id = args::id(t);
        let v = self.life.pack.get("loot_table", &id).unwrap_or_else(|| Arc::new(Value::Null));
        Ok((Some(id), v))
    }

    /// Applies loot functions to a stack ("command" context: source position, source entity),
    /// size bounded.
    fn apply_modifier(&mut self, src: &Src, f: &Value, s: Stack) -> Stack {
        let mut rng = self.life.rng;
        let mut out = {
            let bt = |tag: &str, name: &str| self.block_name_tagged(tag, name);
            let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &bt };
            let view = self.loot_world(PCtx { this: src.entity.map(Who::E), origin: Some(src.pos), ..PCtx::default() });
            let ctx = crate::loot::Ctx { world: Some(&view), ..Default::default() };
            loot.apply(f, s, &ctx, &mut rng)
        };
        self.life.rng = rng;
        let max = self.life.items.max_stack(&out);
        out.count = out.count.min(max);
        out
    }

    // ---- item -------------------------------------------------------------------------------

    pub(crate) fn item_target(&mut self, s: &Src, p: &Parsed, seg: &Seg, kind: &str, arg: &str) -> Result<Holder, Fail> {
        if kind == "block" {
            let pos = self.loaded_pos(s, &p.arg(seg, arg).unwrap_or_default())?;
            if !self.is_container(pos) {
                let k = if arg == "target" { "target" } else { "source" };
                return Err(Fail(tr!(&format!("commands.item.{k}.not_a_container"), pos[0], pos[1], pos[2])));
            }
            Ok(Holder::Block(pos))
        } else {
            Ok(Holder::Ents(self.arg_entities(s, &p.arg(seg, arg).unwrap_or_default(), false)?))
        }
    }

    pub(crate) fn holder_slots(&self, h: &Holder, range: &str) -> Vec<(Option<E>, Vec<Slot>)> {
        match h {
            Holder::Block(pos) => vec![(None, self.block_slot_range(*pos, range))],
            Holder::Ents(es) => es.iter().map(|e| (Some(*e), self.entity_slot_range(*e, range))).collect(),
        }
    }

    pub(crate) fn cmd_item(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let action = w.get(1).copied().unwrap_or("replace");
        let tkind = w.get(2).copied().unwrap_or("entity");
        let target = self.item_target(s, p, seg, tkind, "target")?;
        let range = p.arg(seg, "slots").unwrap_or_default();
        if action == "modify" {
            let modifier = self.arg_item_modifier(&p.arg(seg, "modifier").unwrap_or_default())?;
            let mut selected = 0;
            let mut t_e: Track<E> = Track::default();
            let mut t_b: Track<[i32; 3]> = Track::default();
            let mut view = SlotView::default();
            for (who, slots) in self.holder_slots(&target, &range) {
                let mut n = 0;
                for sl in slots {
                    let Some(cur) = self.command_slot_read(sl, &mut view) else { continue };
                    selected += 1;
                    let new = self.apply_modifier(s, &modifier, cur.clone());
                    if !crate::items::matches(Some(&new), Some(&cur)) || new.count != cur.count {
                        self.command_slot_write(sl, Some(new), &mut view);
                        n += 1;
                    }
                }
                match (who, &target) {
                    (Some(e), _) => t_e.track(e, n),
                    (None, Holder::Block(pos)) => t_b.track(*pos, n),
                    _ => {}
                }
            }
            if selected == 0 {
                return Err(Fail(tr!("commands.item.target.no_such_slot", range)));
            }
            self.broadcast_menus();
            return match target {
                Holder::Block(_) => {
                    let (first, n) = t_b.pick(false);
                    if n == 0 {
                        return Err(Fail(tr!("commands.item.target.failed")));
                    }
                    let pos = first.copied().unwrap_or_default();
                    self.ok_with(s, |_| tr!("commands.item.block.modify.success", t_b.total, pos[0], pos[1], pos[2]));
                    Ok(t_b.total)
                }
                Holder::Ents(_) => {
                    let total = t_b.total + t_e.total;
                    self.track_feedback(
                        s,
                        &t_e,
                        false,
                        Some(Fail(tr!("commands.item.target.failed"))),
                        |sv, e| tr!("commands.item.entity.modify.success.single", total, sv.ent_display(*e)),
                        |n| tr!("commands.item.entity.modify.success.multiple", n),
                    )
                }
            };
        }
        // Items provided: `with <item> [count]` or `from <source> <sourceSlots> [modifier]`.
        let (items, known): (Vec<Stack>, Option<Stack>) = if w.contains(&"with") {
            let count: i32 = p.arg(seg, "count").and_then(|v| v.parse().ok()).unwrap_or(1);
            let st = self.item_input(&p.arg(seg, "item").unwrap_or_default(), count);
            let st = st.ok_or_else(|| Fail(tr!("argument.item.id.invalid", p.arg(seg, "item").unwrap_or_default())))?;
            // More than the maximum stack size = refusal.
            let max = self.life.items.max_stack(&st);
            if count > max {
                return Err(Fail(tr!("arguments.item.overstacked", st.id.clone(), max)));
            }
            (vec![st.clone()], Some(st))
        } else {
            let from = w.iter().position(|x| *x == "from").unwrap_or(0);
            let skind = w.get(from + 1).copied().unwrap_or("entity");
            let source = self.item_target(s, p, seg, skind, "source")?;
            let srange = p.arg(seg, "sourceSlots").unwrap_or_default();
            let modifier = match p.arg(seg, "modifier") {
                Some(m) => Some(self.arg_item_modifier(&m)?),
                None => None,
            };
            let mut out = Vec::new();
            let mut any = false;
            let mut view = SlotView::default();
            for (_, slots) in self.holder_slots(&source, &srange) {
                for sl in slots {
                    any = true;
                    let st = self.command_slot_read(sl, &mut view).unwrap_or_else(|| Stack {
                        id: "minecraft:air".into(),
                        count: 0,
                        components: None,
                    });
                    let st = match &modifier {
                        Some(m) if st.count > 0 => self.apply_modifier(s, m, st),
                        _ => st,
                    };
                    out.push(st);
                }
            }
            if !any {
                return Err(Fail(tr!("commands.item.source.no_such_slot", srange)));
            }
            (out, None)
        };
        // Distribution: `replace` (in order, without remainder), `fill` (in a loop), `override`
        // (in order then empties).
        let mut selected = 0;
        let mut t_e: Track<E> = Track::default();
        let mut t_b: Track<[i32; 3]> = Track::default();
        for (who, slots) in self.holder_slots(&target, &range) {
            let mut n = 0;
            for (next, sl) in slots.into_iter().enumerate() {
                let item = match action {
                    "fill" if !items.is_empty() => Some(items[next % items.len()].clone()),
                    "override" => Some(items.get(next).cloned().unwrap_or_else(|| Stack {
                        id: "minecraft:air".into(),
                        count: 0,
                        components: None,
                    })),
                    _ => items.get(next).cloned(),
                };
                let Some(item) = item else { break };
                selected += 1;
                if self.slot_set(sl, Some(item)) {
                    n += 1;
                }
            }
            match (who, &target) {
                (Some(e), _) => t_e.track(e, n),
                (None, Holder::Block(pos)) => t_b.track(*pos, n),
                _ => {}
            }
        }
        if selected == 0 {
            return Err(Fail(tr!("commands.item.target.no_such_slot", range)));
        }
        self.broadcast_menus();
        let disp = known.as_ref().map(|k| self.stack_display(k));
        match target {
            Holder::Block(pos) => {
                let n = t_b.total;
                if n == 0 {
                    return Err(Fail(match &disp {
                        Some(d) => tr!("commands.item.target.failed.known_item", d.clone()),
                        None => tr!("commands.item.target.failed"),
                    }));
                }
                let msg = match disp {
                    Some(d) => tr!("commands.item.block.replace.success.known_item", n, pos[0], pos[1], pos[2], d),
                    None => tr!("commands.item.block.replace.success", n, pos[0], pos[1], pos[2]),
                };
                self.ok(s, msg);
                Ok(n)
            }
            Holder::Ents(_) => {
                let zero = Fail(match &disp {
                    Some(d) => tr!("commands.item.target.failed.known_item", d.clone()),
                    None => tr!("commands.item.target.failed"),
                });
                let total = t_e.total;
                match disp {
                    Some(d) => {
                        let d2 = d.clone();
                        self.track_feedback(
                            s,
                            &t_e,
                            false,
                            Some(zero),
                            |sv, e| {
                                tr!(
                                    "commands.item.entity.replace.success.single.known_item",
                                    total,
                                    sv.ent_display(*e),
                                    d.clone()
                                )
                            },
                            |n| tr!("commands.item.entity.replace.success.multiple.known_item", n, d2.clone()),
                        )
                    }
                    None => self.track_feedback(
                        s,
                        &t_e,
                        false,
                        Some(zero),
                        |sv, e| tr!("commands.item.entity.replace.success.single", total, sv.ent_display(*e)),
                        |n| tr!("commands.item.entity.replace.success.multiple", n),
                    ),
                }
            }
        }
    }

    // ---- loot -------------------------------------------------------------------------------

    pub(crate) fn cmd_loot(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let si =
            w.iter().enumerate().skip(2).find(|(_, x)| matches!(**x, "fish" | "loot" | "kill" | "mine")).map(|x| x.0);
        let Some(si) = si else { return Err(Fail(tr!("command.failed"))) };
        let tool_of = |sv: &mut Server, p: &Parsed| -> Result<Option<Stack>, Fail> {
            match w
                .get(si + 3)
                .copied()
                .or_else(|| w.get(si + 2).copied().filter(|x| matches!(*x, "mainhand" | "offhand")))
            {
                Some("mainhand") | Some("offhand") => {
                    let main = w.contains(&"mainhand");
                    match s.entity {
                        Some(E::Player) => Ok(sv.slots[if main { 36 + sv.held as usize } else { 45 }].clone()),
                        Some(E::Mob(i)) => {
                            Ok(sv.mobs.mob_ref_any(i).and_then(|m| m.equipment[usize::from(!main)].clone()))
                        }
                        Some(e) => Err(Fail(tr!("commands.drop.no_held_items", sv.ent_display(e)))),
                        None => Err(Fail(tr!("permissions.requires.entity"))),
                    }
                }
                _ => match p.arg(seg, "tool") {
                    Some(t) => Ok(sv.item_input(&t, 1)),
                    None => Ok(None),
                },
            }
        };
        // The game draws the loot from the table's named sequence (`random_sequence`) when it
        // has one, otherwise from the world's random draw: draws stay reproducible from one game
        // to the next, without depending on what the ticks consumed between two commands.
        let seq_of = |v: &Value| v["random_sequence"].as_str().map(str::to_owned);
        let pick = |sv: &mut Server, seq: &Option<String>| match seq {
            Some(q) => *sv.life.seqs.get(q),
            None => sv.life.rng,
        };
        let (drops, table, seq, rng): (Vec<Stack>, Option<String>, Option<String>, crate::rng::Rng) = match w[si] {
            // Chest context (source entity, source position); fishing: the centre of the loaded
            // position given and the tool.
            "loot" | "fish" => {
                let (_, v) = self.arg_loot_table(&p.arg(seg, "loot_table").unwrap_or_default())?;
                let (tool, origin) = if w[si] == "fish" {
                    let pos = self.loaded_pos(s, &p.arg(seg, "pos").unwrap_or_default())?;
                    (tool_of(self, p)?, pos.map(|c| f64::from(c) + 0.5))
                } else {
                    (None, s.pos)
                };
                let seq = seq_of(&v);
                let mut rng = pick(self, &seq);
                let out = self.loot_with(s, &v, tool.as_ref(), None, origin, &mut rng);
                (out, None, seq, rng)
            }
            "kill" => {
                let e = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), false)?;
                let id = match e {
                    E::Mob(i) => {
                        self.mobs.mob_ref_any(i).map(|m| m.death_loot.clone().unwrap_or_else(|| m.kind.loot_table()))
                    }
                    E::Other(_) => {
                        let n = self.ent_nbt(e);
                        n.get("DeathLootTable").and_then(Nbt::as_str).map(str::to_owned).or_else(|| {
                            let ty = self.ent_type(e);
                            Some(format!("minecraft:entities/{}", ty.trim_start_matches("minecraft:")))
                        })
                    }
                    _ => None,
                }
                .filter(|t| self.life.pack.get("loot_table", t).is_some());
                let Some(id) = id else {
                    return Err(Fail(tr!("commands.drop.no_loot_table.entity", self.ent_display(e))));
                };
                let v = self.life.pack.get("loot_table", &id).unwrap_or_else(|| Arc::new(Value::Null));
                let seq = seq_of(&v);
                let mut rng = pick(self, &seq);
                let out = self.loot_with(s, &v, None, Some(e), s.pos, &mut rng);
                (out, Some(id), seq, rng)
            }
            _ => {
                let pos = self.loaded_pos(s, &p.arg(seg, "pos").unwrap_or_default())?;
                let tool = tool_of(self, p)?;
                let st = self.get(pos);
                let name = self.data.block_of(st).map_or("minecraft:air", |b| b.name.as_str()).to_owned();
                let id = format!("minecraft:blocks/{}", name.trim_start_matches("minecraft:"));
                if self.states.is_air(st) || self.life.pack.get("loot_table", &id).is_none() {
                    let bname = tr!(&format!("block.{}", name.replace(':', ".")));
                    return Err(Fail(tr!("commands.drop.no_loot_table.block", bname)));
                }
                let seq = self.life.pack.get("loot_table", &id).and_then(|v| seq_of(&v));
                let mut rng = pick(self, &seq);
                let out = self.block_loot(s, pos, st, tool.as_ref(), &mut rng);
                (out, Some(id), seq, rng)
            }
        };
        match seq {
            Some(q) => *self.life.seqs.get(&q) = rng,
            None => self.life.rng = rng,
        }
        // Targets.
        let mut t: Track<Stack> = Track::default();
        match w[1] {
            "give" => {
                let targets = self.arg_players(s, &p.arg(seg, "players").unwrap_or_default())?;
                for d in &drops {
                    for _ in &targets {
                        let creative = self.game_mode == 1;
                        let before: i32 = self.slots.iter().flatten().map(|x| x.count).sum();
                        let left = crate::menus::inventory_add(
                            &mut self.slots,
                            &self.life.items,
                            self.held as usize,
                            d.clone(),
                            creative,
                        );
                        let after: i32 = self.slots.iter().flatten().map(|x| x.count).sum();
                        if left.is_none() || after > before {
                            t.track(d.clone(), 1);
                        }
                    }
                }
                self.broadcast_menus();
            }
            "spawn" => {
                let pos = args::vec3(self, s, &p.arg(seg, "targetPos").unwrap_or_default());
                for d in &drops {
                    self.spawn_item_at(pos, d.clone());
                    t.track(d.clone(), 1);
                }
            }
            "insert" => {
                let pos = self.loaded_pos(s, &p.arg(seg, "targetPos").unwrap_or_default())?;
                if !self.is_container(pos) {
                    return Err(Fail(tr!("commands.item.target.not_a_container", pos[0], pos[1], pos[2])));
                }
                let size = self.command_container_size(pos);
                let mut view = SlotView::default();
                for d in &drops {
                    let mut st = d.clone();
                    let mut changed = false;
                    for i in 0..size {
                        if st.count <= 0 {
                            break;
                        }
                        // The game reads the slot (drawing sealed loot) before its placement test.
                        let current = self.command_slot_read(Slot::Block(pos, i), &mut view);
                        if !self.container_accepts(pos, i, &st) {
                            continue;
                        }
                        match current {
                            None => {
                                self.command_slot_write(Slot::Block(pos, i), Some(st.clone()), &mut view);
                                changed = true;
                                break;
                            }
                            // Same item: merged as far as the stack allows, and counted as a
                            // change even when the stack was already full (the game's quirk).
                            // The stack grows in place: no slot write, so no write rule runs.
                            Some(mut c) if crate::items::same(&c, &st) && c.count <= self.life.items.max_stack(&c) => {
                                let space = self.life.items.max_stack(&st) - c.count;
                                let n = st.count.min(space);
                                if n > 0 {
                                    c.count += n;
                                    st.count -= n;
                                    self.container_store_item(pos, i, size, Some(c));
                                    self.slot_view_reread(Slot::Block(pos, i), &mut view);
                                }
                                changed = true;
                            }
                            _ => {}
                        }
                    }
                    if changed {
                        // One "changed" mark per drop that went in, like the game.
                        self.container_mark_changed(pos);
                        t.track(d.clone(), 1);
                    }
                }
            }
            _ => {
                // replace entity|block <…> <slot> [count]
                let count: Option<usize> = p.arg(seg, "count").and_then(|v| v.parse().ok());
                let n = count.unwrap_or(drops.len());
                let slot_name = p.arg(seg, "slot").unwrap_or_default();
                let Some(&start) = slot_ids(&slot_name).first() else {
                    return Err(Fail(tr!("slot.unknown", slot_name)));
                };
                let air = Stack { id: "minecraft:air".into(), count: 0, components: None };
                if w[2] == "block" {
                    let pos = self.loaded_pos(s, &p.arg(seg, "targetPos").unwrap_or_default())?;
                    if !self.is_container(pos) {
                        return Err(Fail(tr!("commands.item.target.not_a_container", pos[0], pos[1], pos[2])));
                    }
                    let size = self.command_container_size(pos);
                    if !(0..size as i32).contains(&start) {
                        return Err(Fail(tr!("commands.item.target.no_such_slot", start)));
                    }
                    for i in 0..n {
                        let slot = start as usize + i;
                        let item = drops.get(i).cloned().unwrap_or_else(|| air.clone());
                        let placed = if slot < size {
                            let ok = self.container_accepts(pos, slot, &item);
                            if ok {
                                self.slot_set(Slot::Block(pos, slot), Some(item.clone()));
                            }
                            ok
                        } else {
                            self.past_end(pos, slot, size, &item).map_err(Fail)?
                        };
                        if placed {
                            t.track(item, 1);
                        }
                    }
                } else {
                    // Slots `start`, `start + 1`… of each entity, through its slot access.
                    let ents = self.arg_entities(s, &p.arg(seg, "entities").unwrap_or_default(), false)?;
                    for e in ents {
                        for i in 0..n {
                            let item = drops.get(i).cloned().unwrap_or_else(|| air.clone());
                            let Some(sl) = self.entity_slots(e, [start + i as i32]).first().copied() else { continue };
                            if self.slot_set(sl, Some(item.clone())) {
                                t.track(item, 1);
                            }
                        }
                    }
                    self.broadcast_menus();
                }
            }
        }
        let total = t.total;
        let (first, n) = t.pick(false);
        self.ok_with(s, |sv| match (first, table) {
            (Some(d), Some(tb)) => tr!("commands.drop.success.single_with_table", d.count, sv.stack_display(d), tb),
            (Some(d), None) => tr!("commands.drop.success.single", d.count, sv.stack_display(d)),
            (None, Some(tb)) => tr!("commands.drop.success.multiple_with_table", n, tb),
            (None, None) => tr!("commands.drop.success.multiple", n),
        });
        Ok(total)
    }

    /// Slot `slot` beyond the end of the container at `p` (`loot replace block` with a count that
    /// overflows): the game still tests then writes that slot. A container keeping a single item
    /// ignores it (the jukebox only for a disc, the pot always): `Ok(true)` = counted without
    /// effect; the brewing stand and the chiseled bookshelf refuse at once what they could never
    /// take (`Ok(false)`); everywhere else the read of its list fails and the command stops
    /// with the generic failure, the index error in its tooltip (slots already written stay).
    fn past_end(&self, p: [i32; 3], slot: usize, size: usize, item: &Stack) -> Result<bool, rc_text::Component> {
        let be = self.be_get(p);
        let id = be.and_then(|b| b.get("id")).and_then(Nbt::as_str).unwrap_or("").trim_start_matches("minecraft:");
        let out_of_bounds = |len: usize| {
            let mut c = tr!("command.failed");
            text::hover_text(&mut c, text::lit(format!("Index {slot} out of bounds for length {len}")));
            c
        };
        let empty = item.count <= 0 || item.id == "minecraft:air";
        match id {
            "jukebox" => Ok(!empty && self.stack_has(item, "minecraft:jukebox_playable")),
            "decorated_pot" => Ok(true),
            "brewing_stand" if empty || !self.life.stations.is_potion_input(&self.life.items, item) => Ok(false),
            "chiseled_bookshelf" if empty || !self.life.items.has_tag("minecraft:bookshelf_books", &item.id) => Ok(false),
            // The crafter reads its "triggered" flag just past its 9 slot states.
            "crafter" if slot == size && be.and_then(|b| crate::nbtio::int(b, "triggered")) == Some(1) => Ok(false),
            _ => Err(out_of_bounds(size)),
        }
    }

    /// Draw of `/loot … loot|fish|kill`: the source entity is `this` (killed entity: `this` is
    /// the target and the source is its killer, a player one only when the source is a
    /// player, killed by magic), `origin` the position of the context.
    fn loot_with(
        &mut self,
        src: &Src,
        table: &Value,
        tool: Option<&Stack>,
        killed: Option<E>,
        origin: [f64; 3],
        rng: &mut crate::rng::Rng,
    ) -> Vec<Stack> {
        let bt = |tag: &str, name: &str| self.block_name_tagged(tag, name);
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &bt };
        let (on_fire, baby, sheep) = match killed {
            Some(E::Mob(i)) => self.mobs.mob_ref_any(i).map_or((false, false, None), |m| {
                (
                    m.on_fire(),
                    m.is_baby(),
                    (m.kind == crate::mobs::species::Kind::Sheep).then(|| (dye(m.wool), m.sheared)),
                )
            }),
            _ => (false, false, None),
        };
        let source = src.entity.map(Who::E);
        let by_player = killed.is_some() && src.entity == Some(E::Player);
        let pctx = match killed {
            Some(k) => PCtx {
                this: Some(Who::E(k)),
                origin: Some(origin),
                attacker: source,
                direct_attacker: source,
                attacking_player: source.filter(|_| by_player),
                damage: Some(Dmg::plain("minecraft:magic")),
                ..PCtx::default()
            },
            None => PCtx { this: source, origin: Some(origin), tool: tool.map(Some), ..PCtx::default() },
        };
        let view = self.loot_world(pctx);
        let ctx = crate::loot::Ctx {
            tool: tool.map(Some),
            killed_by_player: by_player,
            on_fire,
            baby,
            damage_type: killed.map(|_| "minecraft:magic".to_owned()),
            sheep,
            world: Some(&view),
            ..Default::default()
        };
        loot.run(table, &ctx, rng)
    }

    /// Draw of `/loot … mine`: block, its block entity and the tool, the source entity as
    /// `this`, origin at the centre of the block.
    fn block_loot(
        &mut self,
        src: &Src,
        pos: [i32; 3],
        st: u32,
        tool: Option<&Stack>,
        rng: &mut crate::rng::Rng,
    ) -> Vec<Stack> {
        let name = self.data.block_of(st).map_or("", |b| b.name.as_str()).to_owned();
        let props = crate::loot::BlockProps::of_state(&self.data, st);
        // The live block entity (a furnace's or brewing stand's NBT lags behind it).
        let be = self.live_block_data(pos);
        let id = format!("minecraft:blocks/{}", name.trim_start_matches("minecraft:"));
        let bt = |tag: &str, n: &str| self.block_name_tagged(tag, n);
        let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &bt };
        let view = self.loot_world(PCtx {
            this: src.entity.map(Who::E),
            origin: Some(pos.map(|c| f64::from(c) + 0.5)),
            block: Some(st),
            tool: Some(tool),
            ..PCtx::default()
        });
        let ctx = crate::loot::Ctx {
            block: Some((name.as_str(), props)),
            tool: Some(tool),
            block_entity: be.as_ref(),
            world: Some(&view),
            ..Default::default()
        };
        loot.drops(&id, &ctx, rng)
    }

    // ---- attribute --------------------------------------------------------------------------

    /// Saved modifiers (identifier, amount, operation) of an attribute: those `/attribute` adds
    /// and removes (the player's and an armor stand's or mannequin's from their compound; every
    /// modifier of a mob).
    fn attr_mods(&self, e: E, attr: &str) -> Vec<(String, f64, i32)> {
        let saved = |list: &[Nbt]| crate::attrs::saved_entry(list, attr).map(|(_, m)| m).unwrap_or_default();
        match e {
            E::Player => saved(crate::nbtio::list(self.pdata.root.get("attributes"))),
            E::Mob(i) => self
                .mobs
                .mob_ref_any(i)
                .and_then(|m| mob_attr(m, attr))
                .map(|a| a.mods.iter().map(|(id, v, op)| ((*id).to_owned(), *v, *op)).collect())
                .unwrap_or_default(),
            E::Other(i) => self.deco_nbt(i).map(|n| saved(crate::nbtio::list(n.get("attributes")))).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Every modifier an attribute holds (saved, equipment, effects, mode), as the lookups of
    /// `/attribute … modifier` see them.
    fn attr_all_mods(&self, e: E, attr: &str) -> Vec<(String, f64, i32)> {
        match e {
            E::Player => self.player_mods(attr),
            E::Other(i) => {
                let mut mods = self.attr_mods(e, attr);
                if let Some(n) = self.deco_nbt(i) {
                    for m in self.nbt_equipment_mods(n).into_iter().filter(|m| m.attr == attr) {
                        mods.retain(|o| o.0 != m.id);
                        mods.push((m.id, m.amount, m.op));
                    }
                }
                mods
            }
            _ => self.attr_mods(e, attr),
        }
    }

    /// Full value of an attribute (base and every modifier, clamped), as the game reads it.
    fn attribute_value(&self, e: E, attr: &str, base: f64) -> f64 {
        match e {
            E::Player => self.player_attribute(attr),
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| mob_attr(m, attr)).map_or(base, crate::mobs::mob::Attr::value),
            E::Other(i) => self.deco_attribute(i, attr).unwrap_or(base),
            _ => base,
        }
    }

    fn set_attr_mods(&mut self, e: E, attr: &str, base: f64, mods: &[(String, f64, i32)]) {
        match e {
            E::Player => {
                let mut list: Vec<Nbt> = crate::nbtio::list(self.pdata.root.get("attributes")).to_vec();
                list.retain(|a| a.get("id").and_then(Nbt::as_str) != Some(attr));
                let ms: Vec<Nbt> = mods
                    .iter()
                    .map(|(id, v, op)| {
                        Nbt::Compound(vec![
                            ("id".into(), Nbt::String(id.clone())),
                            ("amount".into(), Nbt::Double(*v)),
                            ("operation".into(), Nbt::String(op_name(*op).into())),
                        ])
                    })
                    .collect();
                let mut entry = vec![("id".into(), Nbt::String(attr.into())), ("base".into(), Nbt::Double(base))];
                if !ms.is_empty() {
                    entry.push(("modifiers".into(), Nbt::List(ms)));
                }
                list.push(Nbt::Compound(entry));
                crate::nbtio::set(&mut self.pdata.root, "attributes", Nbt::List(list));
                self.send_attributes();
            }
            E::Mob(i) => {
                if let Some(m) = self.mobs.get_mut(i)
                    && let Some(a) = mob_attr_mut(m, attr)
                {
                    a.base = base;
                    a.saved = true;
                    // Modifiers added by the command: permanent (saved).
                    let old: Vec<&'static str> = a.mods.iter().map(|m| m.0).collect();
                    a.mods = mods.iter().map(|(id, v, op)| (intern(id), *v, *op)).collect();
                    let ids: Vec<&'static str> = a.mods.iter().map(|m| m.0).collect();
                    a.perm.retain(|p| ids.contains(p));
                    for id in ids {
                        if !old.contains(&id) && !a.perm.contains(&id) {
                            a.perm.push(id);
                        }
                    }
                    m.needs_sync = true;
                }
            }
            E::Other(i) => self.deco_store_attribute(i, attr, base, mods),
            _ => {}
        }
    }

    pub(crate) fn cmd_attribute(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let e = self.arg_entity(s, &p.arg(seg, "target").unwrap_or_default(), false)?;
        let attr = args::id(&p.arg(seg, "attribute").unwrap_or_default());
        let desc = tr!(&format!("attribute.name.{}", attr.trim_start_matches("minecraft:")));
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        if !self.ent_is_living(e) {
            return Err(Fail(tr!("commands.attribute.failed.entity", self.ent_display(e))));
        }
        let Some(base) = self.attribute_base(e, &attr) else {
            return Err(Fail(tr!("commands.attribute.failed.no_attribute", self.ent_display(e), desc)));
        };
        let scale: f64 = p.arg(seg, "scale").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let mut mods = self.attr_mods(e, &attr);
        let id = p.arg(seg, "id").map(|x| args::id(&x));
        match (w.get(3).copied(), w.get(4).copied(), w.get(5).copied()) {
            (Some("get"), _, _) => {
                let value = self.attribute_value(e, &attr, base);
                let key = "commands.attribute.value.get.success";
                self.ok_with(s, |sv| tr!(key, desc, sv.ent_display(e), text::jd(value)));
                Ok((value * scale) as i32)
            }
            (Some("base"), Some("get"), _) => {
                let key = "commands.attribute.base_value.get.success";
                self.ok_with(s, |sv| tr!(key, desc, sv.ent_display(e), text::jd(base)));
                Ok((base * scale) as i32)
            }
            (Some("base"), Some("set"), _) => {
                let v: f64 = p.arg(seg, "value").and_then(|x| x.parse().ok()).unwrap_or(0.0);
                self.set_attr_mods(e, &attr, v, &mods);
                let key = "commands.attribute.base_value.set.success";
                self.ok_with(s, |sv| tr!(key, desc, sv.ent_display(e), text::jd(v)));
                Ok(1)
            }
            (Some("base"), Some("reset"), _) => {
                let def = self.attribute_default(e, &attr).unwrap_or(base);
                // An armor stand or a mannequin without an entry of its own keeps none (the
                // game resets an existing instance only).
                let held = !matches!(e, E::Other(_))
                    || self.deco_nbt(self.ent_id(e)).is_some_and(|n| {
                        crate::attrs::saved_entry(crate::nbtio::list(n.get("attributes")), &attr).is_some()
                    });
                if held {
                    self.set_attr_mods(e, &attr, def, &mods);
                }
                self.ok(
                    s,
                    tr!("commands.attribute.base_value.reset.success", desc, self.ent_display(e), text::jd(def)),
                );
                Ok(1)
            }
            (Some("modifier"), Some("add"), _) => {
                let id = id.unwrap_or_default();
                if self.attr_all_mods(e, &attr).iter().any(|m| m.0 == id) {
                    return Err(Fail(tr!(
                        "commands.attribute.failed.modifier_already_present",
                        id,
                        desc,
                        self.ent_display(e)
                    )));
                }
                let v: f64 = p.arg(seg, "value").and_then(|x| x.parse().ok()).unwrap_or(0.0);
                let op = match w.get(7).copied() {
                    Some("add_multiplied_base") => 1,
                    Some("add_multiplied_total") => 2,
                    _ => 0,
                };
                mods.push((id.clone(), v, op));
                self.set_attr_mods(e, &attr, base, &mods);
                self.ok_with(s, |sv| tr!("commands.attribute.modifier.add.success", id, desc, sv.ent_display(e)));
                Ok(1)
            }
            (Some("modifier"), Some("remove"), _) => {
                let id = id.unwrap_or_default();
                if !mods.iter().any(|m| m.0 == id) {
                    return Err(Fail(tr!("commands.attribute.failed.no_modifier", desc, self.ent_display(e), id)));
                }
                mods.retain(|m| m.0 != id);
                self.set_attr_mods(e, &attr, base, &mods);
                self.ok_with(s, |sv| tr!("commands.attribute.modifier.remove.success", id, desc, sv.ent_display(e)));
                Ok(1)
            }
            (Some("modifier"), Some("value"), _) => {
                let id = id.unwrap_or_default();
                let all = self.attr_all_mods(e, &attr);
                let Some(m) = all.iter().find(|m| m.0 == id) else {
                    return Err(Fail(tr!("commands.attribute.failed.no_modifier", desc, self.ent_display(e), id)));
                };
                let v = m.1;
                self.ok(
                    s,
                    tr!("commands.attribute.modifier.value.get.success", id, desc, self.ent_display(e), text::jd(v)),
                );
                Ok((v * scale) as i32)
            }
            _ => Err(Fail(tr!("commands.attribute.failed.entity", self.ent_display(e)))),
        }
    }

    /// Default base of an attribute for the entity's type (base reset): the player's table, or
    /// for a mob the base of a freshly built mob of its species (construction values,
    /// before any spawn draw or later change).
    fn attribute_default(&self, e: E, attr: &str) -> Option<f64> {
        match e {
            E::Mob(i) => {
                let kind = self.mobs.mob_ref_any(i)?.kind;
                let fresh = crate::mobs::mob::Mob::new(kind, 0, 0, [0.0; 3], 0.0, crate::mobs::jrand::JRand::new(0));
                mob_attr(&fresh, attr).map(|a| a.base)
            }
            E::Player => crate::attrs::type_default("minecraft:player", attr),
            // Armor stand, mannequin: the table of their type.
            E::Other(i) => {
                let n = self.deco_nbt(i)?;
                crate::attrs::type_default(n.get("id").and_then(Nbt::as_str)?, attr)
            }
            _ => None,
        }
    }

    /// Base of an attribute the entity's type has: its saved base, otherwise the type's
    /// (`None`: the type does not have the attribute).
    fn attribute_base(&self, e: E, attr: &str) -> Option<f64> {
        let saved = |list: &[Nbt]| crate::attrs::saved_entry(list, attr).and_then(|(b, _)| b);
        match e {
            E::Mob(i) => self.mobs.mob_ref_any(i).and_then(|m| mob_attr(m, attr)).map(|a| a.base),
            E::Player => {
                let def = crate::attrs::type_default("minecraft:player", attr)?;
                Some(saved(crate::nbtio::list(self.pdata.root.get("attributes"))).unwrap_or(def))
            }
            E::Other(i) => {
                let def = self.attribute_default(e, attr)?;
                Some(self.deco_nbt(i).and_then(|n| saved(crate::nbtio::list(n.get("attributes")))).unwrap_or(def))
            }
            _ => None,
        }
    }
}

/// Number of slots of a block entity, by identifier.
fn container_size(id: &str) -> usize {
    match id.trim_start_matches("minecraft:") {
        "furnace" | "blast_furnace" | "smoker" => 3,
        "brewing_stand" => 5,
        "hopper" => 5,
        "dispenser" | "dropper" | "crafter" => 9,
        "chiseled_bookshelf" => 6,
        "decorated_pot" => 1,
        "jukebox" => 1,
        "shelf" => 3,
        _ => crate::lootchest::loot_size(id).unwrap_or(27),
    }
}

/// Block entity that is an item container for the commands, by identifier. The lectern and the
/// campfire hold items without being containers in the game (their contents go through `/data`).
fn is_container_id(id: &str) -> bool {
    let short = id.trim_start_matches("minecraft:");
    crate::lootchest::loot_size(id).is_some()
        || matches!(
            short,
            "furnace"
                | "blast_furnace"
                | "smoker"
                | "brewing_stand"
                | "hopper"
                | "dispenser"
                | "dropper"
                | "crafter"
                | "chiseled_bookshelf"
                | "shelf"
                | "decorated_pot"
                | "jukebox"
        )
        || short.ends_with("shulker_box")
}

/// Copies of the slots of a block entity's data in a slot range (`container.*`…), when it is
/// an item container; empty otherwise.
pub(crate) fn be_slot_range(be: &Nbt, range: &str) -> Vec<Option<Stack>> {
    let id = be.get("id").and_then(Nbt::as_str).unwrap_or("");
    if !is_container_id(id) {
        return Vec::new();
    }
    let size = container_size(id);
    // The jukebox and the decorated pot keep their single slot under their own key.
    let items = match id {
        "minecraft:jukebox" => vec![be.get("RecordItem").and_then(Stack::from_nbt)],
        "minecraft:decorated_pot" => vec![be.get("item").and_then(Stack::from_nbt)],
        _ => crate::containers::read_items(be, size),
    };
    slot_ids(range)
        .into_iter()
        .filter_map(|i| usize::try_from(i).ok().filter(|&i| i < size))
        .map(|i| items.get(i).cloned().flatten())
        .collect()
}

fn dye(w: u8) -> String {
    const D: [&str; 16] = [
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];
    D[(w & 15) as usize].to_owned()
}

fn op_name(op: i32) -> &'static str {
    match op {
        1 => "add_multiplied_base",
        2 => "add_multiplied_total",
        _ => "add_value",
    }
}

fn mob_attr<'a>(m: &'a crate::mobs::mob::Mob, attr: &str) -> Option<&'a crate::mobs::mob::Attr> {
    m.attrs.by_name(attr)
}

fn mob_attr_mut<'a>(m: &'a mut crate::mobs::mob::Mob, attr: &str) -> Option<&'a mut crate::mobs::mob::Attr> {
    m.attrs.by_name_mut(attr)
}

/// Modifier identifiers kept for the duration of the process (mob attributes).
fn intern(s: &str) -> &'static str {
    crate::mobs::mob::intern(s)
}
