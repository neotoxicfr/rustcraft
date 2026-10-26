//! Saved hotbars (`docs/specs/creative-inventory.md` § 8): `hotbar.nbt` file of the game folder,
//! uncompressed NBT, stacks in persistent form (`{id, count, components}`).
//!
//! Components are converted between the client's network form of stacks and the persistent
//! form for the common types (enchantments, potions, variants, block states, names…).
//! An unsupported component is kept under a Rustcraft-specific key
//! (`rustcraft:network/<type>`, network bytes): read back identically by Rustcraft, unreadable
//! by the official game (the stack would become empty there, like any invalid stack).

use std::path::PathBuf;

use rc_proto::Reader;
use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, write_varint};
use rc_proto_v777::item::ItemStack;

/// Data version of 26.3.
const DATA_VERSION: i32 = 5023;
const RAW: &str = "rustcraft:network/";

pub type Row = [Option<ItemStack>; 9];

/// Names of the registries useful for the conversion.
pub struct Names<'a> {
    pub items: &'a [String],
    pub components: &'a [String],
    pub potions: &'a [String],
    pub effects: &'a [String],
    pub paintings: Vec<String>,
    pub instruments: Vec<String>,
    pub enchantments: Vec<String>,
    pub banner_patterns: Vec<String>,
}

const DYES: [&str; 16] = [
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

fn id_of(list: &[String], name: &str) -> Option<i32> {
    let full = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
    list.iter().position(|n| *n == full).map(|i| i as i32)
}

fn name_of(list: &[String], id: i32) -> Option<Nbt> {
    usize::try_from(id).ok().and_then(|i| list.get(i)).map(|n| Nbt::String(n.clone()))
}

fn vi(r: &mut Reader<'_>) -> Option<i32> {
    read_varint(r).ok()
}

fn bool_(r: &mut Reader<'_>) -> Option<bool> {
    Some(r.u8().ok()? != 0)
}

/// Persistent value of a component (network → NBT); `None`: not supported.
fn to_persistent(ty: &str, b: &[u8], n: &Names<'_>) -> Option<Nbt> {
    let mut r = Reader::new(b);
    let r = &mut r;
    let v = match ty.strip_prefix("minecraft:")? {
        "damage" | "repair_cost" | "ominous_bottle_amplifier" | "max_damage" | "max_stack_size" => Nbt::Int(vi(r)?),
        "dyed_color" => Nbt::Int(i32::from_be_bytes(r.take(4).ok()?.try_into().ok()?)),
        "rarity" => Nbt::String(RARITIES.get(vi(r)? as usize)?.to_string()),
        "enchantment_glint_override" => Nbt::Byte(i8::from(bool_(r)?)),
        "creative_slot_lock" | "unbreakable" | "glider" | "intangible_projectile" => Nbt::Compound(Vec::new()),
        "custom_name" | "item_name" => rc_proto::nbt::read(r).ok()??,
        "enchantments" | "stored_enchantments" => {
            let k = vi(r)?;
            let mut m = Vec::new();
            for _ in 0..k {
                let (e, l) = (vi(r)?, vi(r)?);
                m.push((n.enchantments.get(e as usize)?.clone(), Nbt::Int(l)));
            }
            Nbt::Compound(m)
        }
        "potion_contents" => {
            let potion = if bool_(r)? { Some(vi(r)?) } else { None };
            if bool_(r)? || vi(r)? != 0 || bool_(r)? {
                return None;
            }
            let mut m = Vec::new();
            if let Some(p) = potion {
                m.push(("potion".into(), name_of(n.potions, p)?));
            }
            Nbt::Compound(m)
        }
        "painting/variant" => name_of(&n.paintings, vi(r)? - 1)?,
        "instrument" => name_of(&n.instruments, vi(r)? - 1)?,
        "fireworks" => {
            let d = vi(r)?;
            if vi(r)? != 0 {
                return None;
            }
            Nbt::Compound(vec![
                ("flight_duration".into(), Nbt::Byte(d as i8)),
                ("explosions".into(), Nbt::List(vec![])),
            ])
        }
        "block_state" => {
            let k = vi(r)?;
            let mut m = Vec::new();
            for _ in 0..k {
                let key = rc_proto::read_str(r, 32767).ok()?.to_owned();
                let val = rc_proto::read_str(r, 32767).ok()?.to_owned();
                m.push((key, Nbt::String(val)));
            }
            Nbt::Compound(m)
        }
        "tooltip_display" => {
            let hide = bool_(r)?;
            let k = vi(r)?;
            let mut l = Vec::new();
            for _ in 0..k {
                l.push(name_of(n.components, vi(r)?)?);
            }
            Nbt::Compound(vec![
                ("hide_tooltip".into(), Nbt::Byte(i8::from(hide))),
                ("hidden_components".into(), Nbt::List(l)),
            ])
        }
        "banner_patterns" => {
            let k = vi(r)?;
            let mut l = Vec::new();
            for _ in 0..k {
                let p = vi(r)? - 1;
                let c = vi(r)?;
                l.push(Nbt::Compound(vec![
                    ("pattern".into(), name_of(&n.banner_patterns, p)?),
                    ("color".into(), Nbt::String(DYES.get(c as usize)?.to_string())),
                ]));
            }
            Nbt::List(l)
        }
        "suspicious_stew_effects" => {
            let k = vi(r)?;
            let mut l = Vec::new();
            for _ in 0..k {
                let e = vi(r)?;
                let d = vi(r)?;
                l.push(Nbt::Compound(vec![("id".into(), name_of(n.effects, e)?), ("duration".into(), Nbt::Int(d))]));
            }
            Nbt::List(l)
        }
        _ => return None,
    };
    (r.rest().is_empty()).then_some(v)
}

fn str_of(v: &Nbt) -> Option<&str> {
    v.as_str()
}

/// Network value of a component (NBT → network); `None`: unreadable.
fn to_network(ty: &str, v: &Nbt, n: &Names<'_>) -> Option<Vec<u8>> {
    let mut w = Vec::new();
    let int = |v: &Nbt| v.as_i64().map(|x| x as i32);
    match ty.strip_prefix("minecraft:")? {
        "damage" | "repair_cost" | "ominous_bottle_amplifier" | "max_damage" | "max_stack_size" => {
            write_varint(&mut w, int(v)?);
        }
        "dyed_color" => w.extend_from_slice(&int(v)?.to_be_bytes()),
        "rarity" => write_varint(&mut w, RARITIES.iter().position(|x| Some(*x) == str_of(v))? as i32),
        "enchantment_glint_override" => w.push(u8::from(int(v)? != 0)),
        "creative_slot_lock" | "unbreakable" | "glider" | "intangible_projectile" => {}
        "custom_name" | "item_name" => rc_proto::nbt::write(&mut w, Some(v)),
        "enchantments" | "stored_enchantments" => {
            let Nbt::Compound(m) = v else { return None };
            write_varint(&mut w, m.len() as i32);
            for (e, l) in m {
                write_varint(&mut w, id_of(&n.enchantments, e)?);
                write_varint(&mut w, int(l)?);
            }
        }
        "potion_contents" => {
            let p = match v {
                Nbt::String(s) => Some(s.as_str()),
                Nbt::Compound(m) => {
                    if m.iter().any(|(k, _)| k != "potion") {
                        return None;
                    }
                    v.get("potion").and_then(Nbt::as_str)
                }
                _ => return None,
            };
            match p {
                Some(p) => {
                    w.push(1);
                    write_varint(&mut w, id_of(n.potions, p)?);
                }
                None => w.push(0),
            }
            w.extend_from_slice(&[0, 0, 0]);
        }
        "painting/variant" => write_varint(&mut w, id_of(&n.paintings, str_of(v)?)? + 1),
        "instrument" => write_varint(&mut w, id_of(&n.instruments, str_of(v)?)? + 1),
        "fireworks" => {
            if v.get("explosions").is_some_and(|e| !matches!(e, Nbt::List(l) if l.is_empty())) {
                return None;
            }
            write_varint(&mut w, v.get("flight_duration").and_then(int).unwrap_or(0));
            write_varint(&mut w, 0);
        }
        "block_state" => {
            let Nbt::Compound(m) = v else { return None };
            write_varint(&mut w, m.len() as i32);
            for (k, x) in m {
                rc_proto::write_str(&mut w, k);
                rc_proto::write_str(&mut w, str_of(x)?);
            }
        }
        "tooltip_display" => {
            w.push(u8::from(v.get("hide_tooltip").and_then(int).unwrap_or(0) != 0));
            let empty = Vec::new();
            let l = match v.get("hidden_components") {
                Some(Nbt::List(l)) => l,
                None => &empty,
                _ => return None,
            };
            write_varint(&mut w, l.len() as i32);
            for c in l {
                write_varint(&mut w, id_of(n.components, str_of(c)?)?);
            }
        }
        "banner_patterns" => {
            let Nbt::List(l) = v else { return None };
            write_varint(&mut w, l.len() as i32);
            for x in l {
                write_varint(&mut w, id_of(&n.banner_patterns, x.get("pattern").and_then(Nbt::as_str)?)? + 1);
                let c = x.get("color").and_then(Nbt::as_str).unwrap_or("white");
                write_varint(&mut w, DYES.iter().position(|d| *d == c)? as i32);
            }
        }
        "suspicious_stew_effects" => {
            let Nbt::List(l) = v else { return None };
            write_varint(&mut w, l.len() as i32);
            for x in l {
                write_varint(&mut w, id_of(n.effects, x.get("id").and_then(Nbt::as_str)?)?);
                write_varint(&mut w, x.get("duration").and_then(int).unwrap_or(160));
            }
        }
        _ => return None,
    }
    Some(w)
}

/// Stack → persistent compound (`{}` if empty).
pub fn stack_to_nbt(s: Option<&ItemStack>, n: &Names<'_>) -> Nbt {
    let Some(s) = s.filter(|s| s.count > 0) else { return Nbt::Compound(Vec::new()) };
    let Some(id) = usize::try_from(s.item).ok().and_then(|i| n.items.get(i)) else {
        return Nbt::Compound(Vec::new());
    };
    let mut out = vec![("id".to_owned(), Nbt::String(id.clone())), ("count".to_owned(), Nbt::Int(s.count))];
    let mut comps = Vec::new();
    for (ty, b) in &s.added {
        let Some(name) = n.components.get(usize::from(*ty)) else { continue };
        match to_persistent(name, b, n) {
            Some(v) => comps.push((name.clone(), v)),
            None => comps.push((format!("{RAW}{name}"), Nbt::ByteArray(b.to_vec()))),
        }
    }
    for ty in &s.removed {
        if let Some(name) = n.components.get(usize::from(*ty)) {
            comps.push((format!("!{name}"), Nbt::Compound(Vec::new())));
        }
    }
    if !comps.is_empty() {
        out.push(("components".to_owned(), Nbt::Compound(comps)));
    }
    Nbt::Compound(out)
}

/// Persistent compound → stack; unreadable or empty: `None`.
pub fn stack_from_nbt(v: &Nbt, n: &Names<'_>) -> Option<ItemStack> {
    let item = id_of(n.items, v.get("id")?.as_str()?)?;
    let count = v.get("count").and_then(Nbt::as_i64).unwrap_or(1) as i32;
    if count <= 0 {
        return None;
    }
    let mut s = ItemStack { item, count, added: Vec::new(), removed: Vec::new() };
    if let Some(Nbt::Compound(m)) = v.get("components") {
        for (k, x) in m {
            if let Some(t) = k.strip_prefix('!') {
                s.removed.push(id_of(n.components, t)? as u16);
            } else if let Some(t) = k.strip_prefix(RAW) {
                let Nbt::ByteArray(b) = x else { return None };
                s.added.push((id_of(n.components, t)? as u16, b.clone().into_boxed_slice()));
            } else {
                let b = to_network(k, x, n)?;
                s.added.push((id_of(n.components, k)? as u16, b.into_boxed_slice()));
            }
        }
    }
    Some(s)
}

/// Saved hotbars, read lazily on first access.
#[derive(Default)]
pub struct Hotbars {
    raw: Option<Nbt>,
}

impl Hotbars {
    fn path() -> PathBuf {
        crate::servers::game_dir().join("hotbar.nbt")
    }

    fn raw(&mut self) -> &Nbt {
        self.raw.get_or_insert_with(|| {
            std::fs::read(Self::path())
                .ok()
                .and_then(|b| rc_proto::nbt::read_named(&mut Reader::new(&b)).ok().flatten())
                .map_or(Nbt::Compound(Vec::new()), |(_, v)| v)
        })
    }

    /// Hotbar `i` (0..9) decoded; list of size ≠ 9: empty hotbar.
    pub fn row(&mut self, i: usize, n: &Names<'_>) -> Row {
        let mut out: Row = Default::default();
        if let Some(Nbt::List(l)) = self.raw().get(&i.to_string())
            && l.len() == 9
        {
            for (o, x) in out.iter_mut().zip(l) {
                *o = stack_from_nbt(x, n);
            }
        }
        out
    }

    /// Empty hotbar: 9 empty compounds (or absent).
    pub fn is_empty(&mut self, i: usize) -> bool {
        match self.raw().get(&i.to_string()) {
            Some(Nbt::List(l)) if l.len() == 9 => l.iter().all(|x| matches!(x, Nbt::Compound(m) if m.is_empty())),
            _ => true,
        }
    }

    /// Saving of hotbar `i` then rewriting of the file.
    pub fn save(&mut self, i: usize, row: &[Option<ItemStack>], n: &Names<'_>) -> std::io::Result<()> {
        let list = Nbt::List(row.iter().map(|s| stack_to_nbt(s.as_ref(), n)).collect());
        self.raw();
        let Some(Nbt::Compound(m)) = &mut self.raw else { return Ok(()) };
        m.retain(|(k, _)| k != "DataVersion" && *k != i.to_string());
        m.insert(0, ("DataVersion".into(), Nbt::Int(DATA_VERSION)));
        m.push((i.to_string(), list));
        let mut w = Vec::new();
        rc_proto::nbt::write_named(&mut w, "", self.raw.as_ref().expect("read"));
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_round_trip() {
        let items = vec!["minecraft:air".to_owned(), "minecraft:painting".into(), "minecraft:enchanted_book".into()];
        let comps = vec![
            "minecraft:painting/variant".to_owned(),
            "minecraft:stored_enchantments".into(),
            "minecraft:lore".into(),
        ];
        let n = Names {
            items: &items,
            components: &comps,
            potions: &[],
            effects: &[],
            paintings: vec!["minecraft:kebab".into(), "minecraft:aztec".into()],
            instruments: vec![],
            enchantments: vec!["minecraft:sharpness".into()],
            banner_patterns: vec![],
        };
        let s = ItemStack {
            item: 2,
            count: 1,
            added: vec![(1, vec![1, 0, 5].into_boxed_slice()), (2, vec![9, 9].into_boxed_slice())],
            removed: vec![],
        };
        let nbt = stack_to_nbt(Some(&s), &n);
        assert_eq!(
            nbt.get("components")
                .and_then(|c| c.get("minecraft:stored_enchantments"))
                .and_then(|e| e.get("minecraft:sharpness")),
            Some(&Nbt::Int(5))
        );
        assert_eq!(stack_from_nbt(&nbt, &n), Some(s));
        let p = ItemStack { item: 1, count: 1, added: vec![(0, vec![2].into_boxed_slice())], removed: vec![] };
        let nbt = stack_to_nbt(Some(&p), &n);
        assert_eq!(
            nbt.get("components").and_then(|c| c.get("minecraft:painting/variant")),
            Some(&Nbt::String("minecraft:aztec".into()))
        );
        assert_eq!(stack_from_nbt(&nbt, &n), Some(p));
        assert_eq!(stack_to_nbt(None, &n), Nbt::Compound(vec![]));
    }
}
