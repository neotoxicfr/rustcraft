//! Network stacks (`item-components.md`): components of the disk form (save NBT)
//! translated into the network encoding that the client decodes, and back (stack placed in creative).
//! Components whose encoding is not translated here are not sent (the stack stays
//! displayed without them); the server and the client compute the same fingerprints, so
//! menu synchronisation stays consistent.

use hashbrown::HashMap;

use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{Decode, Reader};
use rc_proto_v777::container::{ComponentHasher, HashedStack};
use rc_proto_v777::item::ItemStack;

use crate::items::Items;
use crate::player::Stack;

pub const DYE_COLORS: [&str; 16] = [
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
const RARITIES: [&str; 4] = ["common", "uncommon", "rare", "epic"];

/// Registries useful for encoding components.
pub struct NetCodec {
    names: Vec<String>,
    ids: HashMap<String, u16>,
    enchantments: Vec<String>,
    potions: Vec<String>,
    effects: Vec<String>,
    block_entities: Vec<String>,
    entities: Vec<String>,
    /// S7: trim materials and patterns, instruments, banner patterns.
    s7: [Vec<String>; 4],
    pub hasher: ComponentHasher,
}

fn index(list: &[String], name: &str) -> Option<i32> {
    list.iter().position(|n| n == name).map(|i| i as i32)
}

fn vi(w: &mut Vec<u8>, v: i32) {
    write_varint(w, v);
}

/// Block position as the protocol's long integer (x 26 bits, z 26 bits, y 12 bits).
fn pack_block_pos(p: [i32; 3]) -> i64 {
    (i64::from(p[0]) & 0x3FF_FFFF) << 38 | (i64::from(p[2]) & 0x3FF_FFFF) << 12 | (i64::from(p[1]) & 0xFFF)
}

fn unpack_block_pos(v: i64) -> [i32; 3] {
    [(v >> 38) as i32, (v << 52 >> 52) as i32, (v << 26 >> 38) as i32]
}

fn nbt_i32(v: &Nbt) -> Option<i32> {
    v.as_i64().map(|v| v as i32)
}

fn nbt_f32(v: &Nbt) -> Option<f32> {
    match v {
        Nbt::Float(f) => Some(*f),
        Nbt::Double(d) => Some(*d as f32),
        o => o.as_i64().map(|i| i as f32),
    }
}

impl NetCodec {
    pub fn new(data: &rc_data::VersionData, regs: &crate::registries::Registries) -> NetCodec {
        let reg = |n: &str| data.registry(n).map(|r| r.entries.clone()).unwrap_or_default();
        let names = reg("minecraft:data_component_type");
        let ids = names.iter().enumerate().map(|(i, n)| (n.clone(), i as u16)).collect();
        let enchantments = regs.entries("minecraft:enchantment").to_vec();
        let potions = reg("minecraft:potion");
        let hasher =
            ComponentHasher { components: names.clone(), enchantments: enchantments.clone(), potions: potions.clone() };
        NetCodec {
            names,
            ids,
            enchantments,
            potions,
            effects: reg("minecraft:mob_effect"),
            block_entities: reg("minecraft:block_entity_type"),
            entities: reg("minecraft:entity_type"),
            s7: [
                regs.entries("minecraft:trim_material").to_vec(),
                regs.entries("minecraft:trim_pattern").to_vec(),
                regs.entries("minecraft:instrument").to_vec(),
                regs.entries("minecraft:banner_pattern").to_vec(),
            ],
            hasher,
        }
    }

    /// Network value of a component (disk form), `None` if not translated.
    fn encode(&self, items: &Items, name: &str, v: &Nbt, depth: u32) -> Option<Vec<u8>> {
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        let mut w = Vec::new();
        match short {
            "unbreakable" | "creative_slot_lock" | "glider" | "intangible_projectile" => {}
            "max_stack_size" | "max_damage" | "damage" | "repair_cost" | "map_id" | "ominous_bottle_amplifier"
            | "map_post_processing" => vi(&mut w, nbt_i32(v)?),
            "enchantable" => vi(&mut w, nbt_i32(v.get("value").unwrap_or(v))?),
            "rarity" => vi(&mut w, RARITIES.iter().position(|r| Some(*r) == v.as_str())? as i32),
            "base_color" | "dye" => vi(&mut w, DYE_COLORS.iter().position(|r| Some(*r) == v.as_str())? as i32),
            "enchantment_glint_override" => w.push(u8::from(v.as_i64()? != 0)),
            "dyed_color" | "map_color" => w.extend_from_slice(&nbt_i32(v.get("rgb").unwrap_or(v))?.to_be_bytes()),
            "minimum_attack_charge" | "potion_duration_scale" => w.extend_from_slice(&nbt_f32(v)?.to_be_bytes()),
            "custom_name" | "item_name" => rc_proto::nbt::write(&mut w, Some(v)),
            "item_model" | "tooltip_style" | "note_block_sound" => {
                rc_proto::write_str(&mut w, v.as_str()?);
            }
            "lore" => {
                let Nbt::List(l) = v else { return None };
                vi(&mut w, l.len() as i32);
                for line in l {
                    rc_proto::nbt::write(&mut w, Some(line));
                }
            }
            "custom_data" | "map_decorations" | "debug_stick_state" | "recipes" | "lock" | "container_loot"
            | "bucket_entity_data" => rc_proto::nbt::write(&mut w, Some(v)),
            // G5-F10: lodestone compass (optional target: dimension and block; tracked).
            "lodestone_tracker" => {
                let target = v.get("target").and_then(|t| {
                    let dim = t.get("dimension")?.as_str()?;
                    let p = match t.get("pos")? {
                        Nbt::IntArray(a) if a.len() == 3 => [a[0], a[1], a[2]],
                        _ => return None,
                    };
                    Some((dim, p))
                });
                w.push(u8::from(target.is_some()));
                if let Some((dim, p)) = target {
                    rc_proto::write_str(&mut w, dim);
                    w.extend_from_slice(&pack_block_pos(p).to_be_bytes());
                }
                w.push(u8::from(v.get("tracked").and_then(Nbt::as_i64).is_none_or(|t| t != 0)));
            }
            "enchantments" | "stored_enchantments" => {
                let map = v.get("levels").unwrap_or(v);
                let Nbt::Compound(f) = map else { return None };
                let pairs: Vec<(i32, i32)> =
                    f.iter().filter_map(|(k, l)| Some((index(&self.enchantments, k)?, nbt_i32(l)?))).collect();
                vi(&mut w, pairs.len() as i32);
                for (e, l) in pairs {
                    vi(&mut w, e);
                    vi(&mut w, l);
                }
            }
            "potion_contents" => {
                let (potion, color, name, effects) = match v {
                    Nbt::String(p) => (Some(p.as_str()), None, None, None),
                    _ => (
                        v.get("potion").and_then(Nbt::as_str),
                        v.get("custom_color").and_then(nbt_i32),
                        v.get("custom_name").and_then(Nbt::as_str),
                        v.get("custom_effects"),
                    ),
                };
                match potion.and_then(|p| index(&self.potions, p)) {
                    Some(p) => {
                        w.push(1);
                        vi(&mut w, p);
                    }
                    None => w.push(0),
                }
                match color {
                    Some(c) => {
                        w.push(1);
                        w.extend_from_slice(&c.to_be_bytes());
                    }
                    None => w.push(0),
                }
                let effects = crate::nbtio::list(effects);
                vi(&mut w, effects.len() as i32);
                for e in effects {
                    self.effect(&mut w, e)?;
                }
                match name {
                    Some(n) => {
                        w.push(1);
                        rc_proto::write_str(&mut w, n);
                    }
                    None => w.push(0),
                }
            }
            "block_entity_data" | "entity_data" => {
                let id = v.get("id").and_then(Nbt::as_str)?;
                let reg = if short == "block_entity_data" { &self.block_entities } else { &self.entities };
                vi(&mut w, index(reg, id)?);
                let mut rest = v.clone();
                crate::nbtio::remove(&mut rest, "id");
                rc_proto::nbt::write(&mut w, Some(&rest));
            }
            "block_state" => {
                let Nbt::Compound(f) = v else { return None };
                vi(&mut w, f.len() as i32);
                for (k, s) in f {
                    rc_proto::write_str(&mut w, k);
                    rc_proto::write_str(&mut w, s.as_str()?);
                }
            }
            "tooltip_display" => {
                w.push(u8::from(v.get("hide_tooltip").and_then(Nbt::as_i64).unwrap_or(0) != 0));
                let hidden: Vec<i32> = crate::nbtio::list(v.get("hidden_components"))
                    .iter()
                    .filter_map(|n| self.ids.get(n.as_str()?).map(|&i| i32::from(i)))
                    .collect();
                vi(&mut w, hidden.len() as i32);
                for h in hidden {
                    vi(&mut w, h);
                }
            }
            "container" => {
                if depth > 8 {
                    return None;
                }
                let mut slots: Vec<(usize, &Nbt)> = crate::nbtio::list(Some(v))
                    .iter()
                    .filter_map(|e| Some((e.get("slot").and_then(Nbt::as_i64)? as usize, e.get("item")?)))
                    .filter(|(s, _)| *s < 256)
                    .collect();
                slots.sort_by_key(|s| s.0);
                let n = slots.last().map_or(0, |s| s.0 + 1);
                vi(&mut w, n as i32);
                let mut it = slots.into_iter().peekable();
                for i in 0..n {
                    match it.peek() {
                        Some((s, item)) if *s == i => {
                            let st = Stack::from_nbt(item)?;
                            w.push(1);
                            self.template(items, &mut w, &st, depth + 1)?;
                            it.next();
                        }
                        _ => w.push(0),
                    }
                }
            }
            // S8 (and F07, pillagers' crossbow): charged projectiles of the crossbow (list of
            // templates, without a presence marker).
            "charged_projectiles" => {
                if depth > 8 {
                    return None;
                }
                let l = crate::nbtio::list(Some(v));
                vi(&mut w, l.len().min(1024) as i32);
                for e in l.iter().take(1024) {
                    let st = Stack::from_nbt(e)?;
                    self.template(items, &mut w, &st, depth + 1)?;
                }
            }
            // S7: trim, horn, suspicious stew, fireworks, banners, decorated pot.
            "trim" => {
                vi(&mut w, index(&self.s7[0], v.get("material")?.as_str()?)? + 1);
                vi(&mut w, index(&self.s7[1], v.get("pattern")?.as_str()?)? + 1);
            }
            "instrument" => vi(&mut w, index(&self.s7[2], v.as_str()?)? + 1),
            "suspicious_stew_effects" => {
                let l = crate::nbtio::list(Some(v));
                vi(&mut w, l.len() as i32);
                for e in l {
                    vi(&mut w, index(&self.effects, e.get("id")?.as_str()?)?);
                    vi(&mut w, e.get("duration").and_then(nbt_i32).unwrap_or(160));
                }
            }
            "firework_explosion" => self.explosion(&mut w, v)?,
            "fireworks" => {
                vi(&mut w, v.get("flight_duration").and_then(nbt_i32).unwrap_or(0));
                let l = crate::nbtio::list(v.get("explosions"));
                vi(&mut w, l.len().min(256) as i32);
                for e in l.iter().take(256) {
                    self.explosion(&mut w, e)?;
                }
            }
            "banner_patterns" => {
                let l = crate::nbtio::list(Some(v));
                vi(&mut w, l.len() as i32);
                for e in l {
                    vi(&mut w, index(&self.s7[3], e.get("pattern")?.as_str()?)? + 1);
                    vi(&mut w, DYE_COLORS.iter().position(|c| Some(*c) == e.get("color").and_then(Nbt::as_str))? as i32);
                }
            }
            "pot_decorations" => {
                for side in ["back", "left", "right", "front"] {
                    let id = v.get(side).and_then(|s| s.as_str().or_else(|| s.get("id")?.as_str()));
                    match id {
                        Some(id) => {
                            w.push(1);
                            let st = Stack { id: id.to_owned(), count: 1, components: None };
                            self.template(items, &mut w, &st, depth + 1)?;
                        }
                        None => w.push(0),
                    }
                }
            }
            _ => return None,
        }
        Some(w)
    }

    /// Firework explosion: shape, colours, fade colours, trail, twinkle.
    fn explosion(&self, w: &mut Vec<u8>, e: &Nbt) -> Option<()> {
        const SHAPES: [&str; 5] = ["small_ball", "large_ball", "star", "creeper", "burst"];
        let shape = e.get("shape").and_then(Nbt::as_str).unwrap_or("small_ball");
        vi(w, SHAPES.iter().position(|s| *s == shape)? as i32);
        for k in ["colors", "fade_colors"] {
            let c: Vec<i32> = match e.get(k) {
                Some(Nbt::IntArray(a)) => a.clone(),
                Some(Nbt::List(l)) => l.iter().filter_map(nbt_i32).collect(),
                _ => Vec::new(),
            };
            vi(w, c.len() as i32);
            for x in c {
                w.extend_from_slice(&x.to_be_bytes());
            }
        }
        let flag = |k: &str| e.get(k).and_then(Nbt::as_i64).is_some_and(|v| v != 0);
        w.push(u8::from(flag("has_trail")));
        w.push(u8::from(flag("has_twinkle")));
        Some(())
    }

    fn effect(&self, w: &mut Vec<u8>, e: &Nbt) -> Option<()> {
        vi(w, index(&self.effects, e.get("id")?.as_str()?)?);
        self.effect_details(w, e, 0)
    }

    fn effect_details(&self, w: &mut Vec<u8>, e: &Nbt, depth: u32) -> Option<()> {
        let flag = |k: &str, d: bool| e.get(k).and_then(Nbt::as_i64).map_or(d, |v| v != 0);
        vi(w, e.get("amplifier").and_then(nbt_i32).unwrap_or(0));
        vi(w, e.get("duration").and_then(nbt_i32).unwrap_or(1));
        w.push(u8::from(flag("ambient", false)));
        let visible = flag("show_particles", true);
        w.push(u8::from(visible));
        w.push(u8::from(flag("show_icon", visible)));
        match e.get("hidden_effect") {
            Some(h) if depth < 8 => {
                w.push(1);
                self.effect_details(w, h, depth + 1)?;
            }
            _ => w.push(0),
        }
        Some(())
    }

    /// Stack template (item, quantity, patch).
    fn template(&self, items: &Items, w: &mut Vec<u8>, s: &Stack, depth: u32) -> Option<()> {
        vi(w, items.id(&s.id)?);
        vi(w, s.count);
        let (added, removed) = self.patch(items, s, depth);
        vi(w, added.len() as i32);
        vi(w, removed.len() as i32);
        for (ty, b) in &added {
            vi(w, i32::from(*ty));
            w.extend_from_slice(b);
        }
        for ty in removed {
            vi(w, i32::from(ty));
        }
        Some(())
    }

    #[allow(clippy::type_complexity)]
    fn patch(&self, items: &Items, s: &Stack, depth: u32) -> (Vec<(u16, Box<[u8]>)>, Vec<u16>) {
        let (mut added, mut removed) = (Vec::new(), Vec::new());
        if let Some(Nbt::Compound(f)) = &s.components {
            for (k, v) in f {
                if let Some(name) = k.strip_prefix('!') {
                    if let Some(&ty) = self.ids.get(name) {
                        removed.push(ty);
                    }
                } else if let Some(&ty) = self.ids.get(k.as_str())
                    && let Some(b) = self.encode(items, k, v, depth)
                {
                    added.push((ty, b.into_boxed_slice()));
                }
            }
        }
        (added, removed)
    }

    /// Full network stack.
    pub fn to_net(&self, items: &Items, s: &Stack) -> Option<ItemStack> {
        if s.count <= 0 {
            return None;
        }
        let item = items.id(&s.id)?;
        let (added, removed) = self.patch(items, s, 0);
        Some(ItemStack { item, count: s.count, added, removed })
    }

    pub fn hashed(&self, items: &Items, s: Option<&Stack>) -> Option<HashedStack> {
        s.and_then(|s| self.to_net(items, s)).map(|n| self.hasher.stack(&n))
    }

    /// Network stack → disk form (translated components only).
    pub fn from_net(&self, items: &Items, s: &ItemStack) -> Option<Stack> {
        let id = items.name(s.item)?.to_owned();
        if s.count <= 0 || id == "minecraft:air" {
            return None;
        }
        let mut fields = Vec::new();
        for (ty, bytes) in &s.added {
            let Some(name) = self.names.get(usize::from(*ty)) else { continue };
            if let Some(v) = self.decode(items, name, bytes) {
                fields.push((name.clone(), v));
            }
        }
        for ty in &s.removed {
            if let Some(name) = self.names.get(usize::from(*ty)) {
                fields.push((format!("!{name}"), Nbt::Compound(Vec::new())));
            }
        }
        let mut st = Stack { id, count: s.count, components: (!fields.is_empty()).then_some(Nbt::Compound(fields)) };
        crate::items::normalize_patch(&mut st);
        Some(st)
    }

    fn decode(&self, _items: &Items, name: &str, bytes: &[u8]) -> Option<Nbt> {
        let short = name.strip_prefix("minecraft:").unwrap_or(name);
        let mut r = Reader::new(bytes);
        let r = &mut r;
        let v = match short {
            "unbreakable" | "creative_slot_lock" | "glider" | "intangible_projectile" => Nbt::Compound(Vec::new()),
            "max_stack_size" | "max_damage" | "damage" | "repair_cost" | "map_id" | "ominous_bottle_amplifier" => {
                Nbt::Int(read_varint(r).ok()?)
            }
            "enchantable" => Nbt::Compound(vec![("value".into(), Nbt::Int(read_varint(r).ok()?))]),
            "rarity" => Nbt::String(RARITIES.get(read_varint(r).ok()? as usize)?.to_string()),
            "base_color" | "dye" => Nbt::String(DYE_COLORS.get(read_varint(r).ok()? as usize)?.to_string()),
            "enchantment_glint_override" => Nbt::Byte(i8::from(bool::decode(r).ok()?)),
            "dyed_color" | "map_color" => Nbt::Int(i32::decode(r).ok()?),
            "minimum_attack_charge" | "potion_duration_scale" => Nbt::Float(f32::decode(r).ok()?),
            "custom_name" | "item_name" => rc_proto::nbt::read(r).ok()??,
            "item_model" | "tooltip_style" | "note_block_sound" => {
                Nbt::String(rc_proto::read_str(r, 32767).ok()?.to_owned())
            }
            "lore" => {
                let n = read_varint(r).ok()?.clamp(0, 256);
                let mut l = Vec::new();
                for _ in 0..n {
                    l.push(rc_proto::nbt::read(r).ok()??);
                }
                Nbt::List(l)
            }
            "custom_data" | "map_decorations" | "debug_stick_state" | "recipes" | "lock" | "container_loot"
            | "bucket_entity_data" => rc_proto::nbt::read(r).ok()??,
            "lodestone_tracker" => {
                let mut f = Vec::new();
                if bool::decode(r).ok()? {
                    let dim = rc_proto::read_str(r, 32767).ok()?.to_owned();
                    let p = unpack_block_pos(i64::decode(r).ok()?);
                    f.push((
                        "target".into(),
                        Nbt::Compound(vec![
                            ("dimension".into(), Nbt::String(dim)),
                            ("pos".into(), Nbt::IntArray(p.to_vec())),
                        ]),
                    ));
                }
                if !bool::decode(r).ok()? {
                    f.push(("tracked".into(), Nbt::Byte(0)));
                }
                Nbt::Compound(f)
            }
            "enchantments" | "stored_enchantments" => {
                let n = read_varint(r).ok()?.clamp(0, 256);
                let mut f = Vec::new();
                for _ in 0..n {
                    let e = self.enchantments.get(read_varint(r).ok()? as usize)?.clone();
                    f.push((e, Nbt::Int(read_varint(r).ok()?)));
                }
                Nbt::Compound(f)
            }
            "potion_contents" => {
                let mut f = Vec::new();
                if bool::decode(r).ok()? {
                    let p = self.potions.get(read_varint(r).ok()? as usize)?.clone();
                    f.push(("potion".into(), Nbt::String(p)));
                }
                if bool::decode(r).ok()? {
                    f.push(("custom_color".into(), Nbt::Int(i32::decode(r).ok()?)));
                }
                if read_varint(r).ok()? != 0 {
                    return None;
                }
                if bool::decode(r).ok()? {
                    f.push(("custom_name".into(), Nbt::String(rc_proto::read_str(r, 32767).ok()?.to_owned())));
                }
                Nbt::Compound(f)
            }
            "block_entity_data" | "entity_data" => {
                let reg = if short == "block_entity_data" { &self.block_entities } else { &self.entities };
                let id = reg.get(read_varint(r).ok()? as usize)?.clone();
                let mut v = rc_proto::nbt::read(r).ok()??;
                if let Nbt::Compound(f) = &mut v {
                    f.insert(0, ("id".into(), Nbt::String(id)));
                }
                v
            }
            "block_state" => {
                let n = read_varint(r).ok()?.clamp(0, 256);
                let mut f = Vec::new();
                for _ in 0..n {
                    let k = rc_proto::read_str(r, 32767).ok()?.to_owned();
                    f.push((k, Nbt::String(rc_proto::read_str(r, 32767).ok()?.to_owned())));
                }
                Nbt::Compound(f)
            }
            _ => return None,
        };
        r.is_empty().then_some(v)
    }
}
