//! Container screen packets (`docs/specs/inventories.md` § 1) and hashed stacks of the
//! click (§ 1.6, 1.7).

use rc_proto::nbt::Nbt;
use rc_proto::varint::{read_varint, write_varint};
use rc_proto::{BlockPos, Decode, Encode, Reader, Result};

use crate::item::ItemStack;

/// `open_screen`: window, menu type (registry `menu`), title.
pub fn read_open_screen(r: &mut Reader<'_>) -> Result<(i32, i32, Option<Nbt>)> {
    let window = read_varint(r)?;
    let kind = read_varint(r)?;
    Ok((window, kind, rc_proto::nbt::read(r)?))
}

/// `container_set_data`: window, property, value.
pub fn read_set_data(r: &mut Reader<'_>) -> Result<(i32, i16, i16)> {
    Ok((read_varint(r)?, i16::decode(r)?, i16::decode(r)?))
}

pub fn write_container_close(w: &mut Vec<u8>, window: i32) {
    write_varint(w, window);
}

/// Hashed stack: item, quantity, added components (type, fingerprint) and removed ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedStack {
    pub item: i32,
    pub count: i32,
    pub added: Vec<(u16, i32)>,
    pub removed: Vec<u16>,
}

fn write_hashed(w: &mut Vec<u8>, s: Option<&HashedStack>) {
    let Some(s) = s else {
        w.push(0);
        return;
    };
    w.push(1);
    write_varint(w, s.item);
    write_varint(w, s.count);
    write_varint(w, s.added.len() as i32);
    for &(ty, h) in &s.added {
        write_varint(w, i32::from(ty));
        h.encode(w);
    }
    write_varint(w, s.removed.len() as i32);
    for &ty in &s.removed {
        write_varint(w, i32::from(ty));
    }
}

/// `container_click` (§ 1.6).
#[allow(clippy::too_many_arguments)]
pub fn write_container_click(
    w: &mut Vec<u8>,
    window: i32,
    state: i32,
    slot: i16,
    button: i8,
    mode: i32,
    changed: &[(i16, Option<HashedStack>)],
    carried: Option<&HashedStack>,
) {
    write_varint(w, window);
    write_varint(w, state);
    slot.encode(w);
    button.encode(w);
    write_varint(w, mode);
    write_varint(w, changed.len().min(128) as i32);
    for (slot, s) in changed.iter().take(128) {
        slot.encode(w);
        write_hashed(w, s.as_ref());
    }
    write_hashed(w, carried);
}

/// `use_item_on`: hand, targeted block, face (`Direction` order), targeted point relative to the block,
/// head in the block, sequence number.
pub fn write_use_item_on(w: &mut Vec<u8>, hand: i32, pos: [i32; 3], face: i32, at: [f32; 3], inside: bool, seq: i32) {
    write_varint(w, hand);
    BlockPos { x: pos[0], y: pos[1], z: pos[2] }.encode(w);
    write_varint(w, face);
    for v in at {
        v.encode(w);
    }
    inside.encode(w);
    false.encode(w);
    write_varint(w, seq);
}

// --- Fingerprints (§ 1.7) ---

const CRC32C_TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0x82F6_3B78 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

pub fn crc32c(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c = CRC32C_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// Fingerprints of the nodes of the persistent form.
pub mod node {
    use super::crc32c;

    pub fn int(v: i32) -> u32 {
        let mut b = [8u8; 5];
        b[1..].copy_from_slice(&v.to_le_bytes());
        crc32c(&b)
    }
    pub fn byte(v: i8) -> u32 {
        crc32c(&[6, v as u8])
    }
    pub fn short(v: i16) -> u32 {
        let s = v.to_le_bytes();
        crc32c(&[7, s[0], s[1]])
    }
    pub fn long(v: i64) -> u32 {
        let mut b = [9u8; 9];
        b[1..].copy_from_slice(&v.to_le_bytes());
        crc32c(&b)
    }
    pub fn float(v: f32) -> u32 {
        let mut b = [0x0Au8; 5];
        b[1..].copy_from_slice(&v.to_bits().to_le_bytes());
        crc32c(&b)
    }
    pub fn double(v: f64) -> u32 {
        let mut b = [0x0Bu8; 9];
        b[1..].copy_from_slice(&v.to_bits().to_le_bytes());
        crc32c(&b)
    }
    pub fn boolean(v: bool) -> u32 {
        crc32c(&[0x0D, u8::from(v)])
    }
    pub fn string(s: &str) -> u32 {
        let mut b = Vec::with_capacity(5 + s.len() * 2);
        b.push(0x0C);
        let n = s.encode_utf16().count() as i32;
        b.extend_from_slice(&n.to_le_bytes());
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_le_bytes());
        }
        crc32c(&b)
    }
    pub fn list(items: &[u32]) -> u32 {
        let mut b = Vec::with_capacity(2 + items.len() * 4);
        b.push(4);
        for h in items {
            b.extend_from_slice(&h.to_le_bytes());
        }
        b.push(5);
        crc32c(&b)
    }
    /// Table: entries (H(key), H(value)) sorted before hashing.
    pub fn map(entries: &mut [(u32, u32)]) -> u32 {
        entries.sort_unstable();
        let mut b = Vec::with_capacity(2 + entries.len() * 8);
        b.push(2);
        for (k, v) in entries.iter() {
            b.extend_from_slice(&k.to_le_bytes());
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.push(3);
        crc32c(&b)
    }
    pub fn bytes(v: &[u8]) -> u32 {
        let mut b = Vec::with_capacity(v.len() + 2);
        b.push(0x0E);
        b.extend_from_slice(v);
        b.push(0x0F);
        crc32c(&b)
    }
    pub fn ints(v: &[i32]) -> u32 {
        let mut b = vec![0x10];
        for x in v {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.push(0x11);
        crc32c(&b)
    }
    pub fn longs(v: &[i64]) -> u32 {
        let mut b = vec![0x12];
        for x in v {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.push(0x13);
        crc32c(&b)
    }
}

/// Boolean keys of a text's style (NBT: byte; persistent form: boolean).
const TEXT_BOOLS: [&str; 6] = ["bold", "italic", "underlined", "strikethrough", "obfuscated", "interpret"];

/// Fingerprint of a received NBT value (approximate persistent form: the bytes of the style
/// keys become booleans).
fn hash_nbt(v: &Nbt, key: Option<&str>) -> u32 {
    match v {
        Nbt::Byte(b) if key.is_some_and(|k| TEXT_BOOLS.contains(&k)) => node::boolean(*b != 0),
        Nbt::Byte(b) => node::byte(*b),
        Nbt::Short(s) => node::short(*s),
        Nbt::Int(i) => node::int(*i),
        Nbt::Long(l) => node::long(*l),
        Nbt::Float(f) => node::float(*f),
        Nbt::Double(d) => node::double(*d),
        Nbt::ByteArray(b) => node::bytes(b),
        Nbt::String(s) => node::string(s),
        Nbt::List(l) => node::list(&l.iter().map(|e| hash_nbt(e, None)).collect::<Vec<_>>()),
        Nbt::Compound(c) => {
            let mut e: Vec<_> = c.iter().map(|(k, v)| (node::string(k), hash_nbt(v, Some(k)))).collect();
            node::map(&mut e)
        }
        Nbt::IntArray(a) => node::ints(a),
        Nbt::LongArray(a) => node::longs(a),
    }
}

/// Text component: string if it is an unstyled literal with no children.
fn hash_text(v: &Nbt) -> u32 {
    if let Nbt::Compound(c) = v
        && let [(k, Nbt::String(s))] = &c[..]
        && k == "text"
    {
        return node::string(s);
    }
    hash_nbt(v, None)
}

/// Computation of component fingerprints from their network bytes.
#[derive(Debug, Clone, Default)]
pub struct ComponentHasher {
    /// Registry `data_component_type` (full names).
    pub components: Vec<String>,
    /// Synced registry `enchantment` (full names).
    pub enchantments: Vec<String>,
    /// Registry `potion` (full names).
    pub potions: Vec<String>,
}

const RARITIES: [&str; 4] = ["common", "uncommon", "rare", "epic"];

impl ComponentHasher {
    /// Fingerprint of a component; `None` if its persistent form is not known here (the
    /// server will then send the slot back, `inventories.md` § 6.1).
    pub fn hash(&self, ty: u16, bytes: &[u8]) -> Option<i32> {
        let name = self.components.get(usize::from(ty))?;
        let name = name.strip_prefix("minecraft:").unwrap_or(name);
        let mut r = Reader::new(bytes);
        let h = match name {
            "max_stack_size" | "max_damage" | "damage" | "repair_cost" | "map_id" | "ominous_bottle_amplifier" => {
                node::int(read_varint(&mut r).ok()?)
            }
            "unbreakable" | "creative_slot_lock" | "glider" | "intangible_projectile" => node::map(&mut []),
            "enchantment_glint_override" => node::boolean(bool::decode(&mut r).ok()?),
            "rarity" => node::string(RARITIES.get(read_varint(&mut r).ok()? as usize).copied().unwrap_or("common")),
            "custom_name" | "item_name" => hash_text(&rc_proto::nbt::read(&mut r).ok()??),
            "lore" => {
                let n = read_varint(&mut r).ok()?;
                let mut lines = Vec::with_capacity(n.clamp(0, 256) as usize);
                for _ in 0..n {
                    lines.push(hash_text(&rc_proto::nbt::read(&mut r).ok()??));
                }
                node::list(&lines)
            }
            "enchantments" | "stored_enchantments" => {
                let n = read_varint(&mut r).ok()?;
                let mut e = Vec::with_capacity(n.clamp(0, 256) as usize);
                for _ in 0..n {
                    let id = read_varint(&mut r).ok()?;
                    let level = read_varint(&mut r).ok()?;
                    let name = self.enchantments.get(usize::try_from(id).ok()?)?;
                    e.push((node::string(name), node::int(level)));
                }
                node::map(&mut e)
            }
            "dyed_color" | "map_color" => node::int(i32::decode(&mut r).ok()?),
            // Potion with no custom effects: { potion, custom_color, custom_name }.
            "potion_contents" => {
                let mut e = Vec::new();
                if bool::decode(&mut r).ok()? {
                    let id = usize::try_from(read_varint(&mut r).ok()?).ok()?;
                    e.push((node::string("potion"), node::string(self.potions.get(id)?)));
                }
                if bool::decode(&mut r).ok()? {
                    e.push((node::string("custom_color"), node::int(i32::decode(&mut r).ok()?)));
                }
                if read_varint(&mut r).ok()? != 0 {
                    return None;
                }
                if bool::decode(&mut r).ok()? {
                    let name = rc_proto::read_str(&mut r, 32767).ok()?;
                    e.push((node::string("custom_name"), node::string(name)));
                }
                node::map(&mut e)
            }
            _ => return None,
        };
        r.is_empty().then_some(h as i32)
    }

    /// Hashed stack; component of unknown form: fingerprint 0 (the server will correct it).
    pub fn stack(&self, s: &ItemStack) -> HashedStack {
        HashedStack {
            item: s.item,
            count: s.count,
            added: s.added.iter().map(|(ty, b)| (*ty, self.hash(*ty, b).unwrap_or(0))).collect(),
            removed: s.removed.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32c_reference() {
        // Classic CRC-32C check value.
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn node_bytes() {
        assert_eq!(node::int(5), crc32c(&[8, 5, 0, 0, 0]));
        assert_eq!(node::string("a"), crc32c(&[0x0C, 1, 0, 0, 0, b'a', 0]));
        assert_eq!(node::map(&mut []), crc32c(&[2, 3]));
        assert_eq!(node::boolean(true), crc32c(&[0x0D, 1]));
        let (a, b) = (node::int(1), node::int(2));
        let mut l = vec![4];
        l.extend_from_slice(&a.to_le_bytes());
        l.extend_from_slice(&b.to_le_bytes());
        l.push(5);
        assert_eq!(node::list(&[a, b]), crc32c(&l));
    }

    #[test]
    fn map_entries_are_sorted() {
        let k1 = (node::string("x"), node::int(1));
        let k2 = (node::string("y"), node::int(2));
        assert_eq!(node::map(&mut [k1, k2]), node::map(&mut [k2, k1]));
    }

    fn hasher() -> ComponentHasher {
        let components = ["custom_data", "max_stack_size", "max_damage", "damage", "unbreakable", "custom_name"]
            .iter()
            .map(|n| format!("minecraft:{n}"))
            .chain(["minecraft:enchantments".to_owned()])
            .collect();
        ComponentHasher {
            components,
            enchantments: vec!["minecraft:aqua_affinity".into(), "minecraft:sharpness".into()],
            potions: Vec::new(),
        }
    }

    #[test]
    fn component_hashes() {
        let h = hasher();
        assert_eq!(h.hash(3, &[5]), Some(node::int(5) as i32));
        assert_eq!(h.hash(4, &[]), Some(crc32c(&[2, 3]) as i32));
        // enchantments : { "minecraft:sharpness": 3 }
        let mut e = [(node::string("minecraft:sharpness"), node::int(3))];
        assert_eq!(h.hash(6, &[1, 1, 3]), Some(node::map(&mut e) as i32));
        // Literal custom_name (root string NBT) → string.
        let mut b = vec![8];
        b.extend_from_slice(&[0, 3]);
        b.extend_from_slice(b"Bob");
        assert_eq!(h.hash(5, &b), Some(node::string("Bob") as i32));
        // Unknown form: no fingerprint.
        assert_eq!(h.hash(0, &[]), None);
    }

    #[test]
    fn click_packet_layout() {
        let s = HashedStack { item: 7, count: 2, added: vec![(3, 0x0102_0304)], removed: vec![4] };
        let mut w = Vec::new();
        write_container_click(&mut w, 1, 5, 12, 0, 0, &[(12, None)], Some(&s));
        assert_eq!(w, [1, 5, 0, 12, 0, 0, 1, 0, 12, 0, 1, 7, 2, 1, 3, 1, 2, 3, 4, 1, 4]);
    }
}
