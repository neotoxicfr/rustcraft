//! Special screen packets (`docs/specs/special-screens.md`): trading, mounts, books,
//! signs, beacon, anvil, workstation blocks and operator screens.

use rc_proto::varint::{read_varint, write_varint, write_varlong};
use rc_proto::{BlockPos, Decode, Encode, Reader, Result, write_str};

use crate::item::{ItemCodec, ItemStack};

/// Merchant offer (§ 1.1).
#[derive(Debug, Clone, PartialEq)]
pub struct Offer {
    pub cost_a: ItemStack,
    pub result: Option<ItemStack>,
    pub cost_b: Option<ItemStack>,
    pub out_of_stock: bool,
    pub uses: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub special_diff: i32,
    pub price_multiplier: f32,
    pub demand: i32,
}

/// `merchant_offers`: window, offers, level, experience, progress, restock.
#[derive(Debug, Clone, PartialEq)]
pub struct MerchantOffers {
    pub window: i32,
    pub offers: Vec<Offer>,
    pub level: i32,
    pub xp: i32,
    pub show_progress: bool,
    pub can_restock: bool,
}

pub fn read_merchant_offers(items: &ItemCodec, r: &mut Reader<'_>) -> Result<MerchantOffers> {
    let window = read_varint(r)?;
    let n = rc_proto::read_count(r, 4)?;
    let mut offers = Vec::with_capacity(n);
    for _ in 0..n {
        let cost_a = items.read_cost(r)?;
        let result = items.read_stack(r)?;
        let cost_b = if bool::decode(r)? { Some(items.read_cost(r)?) } else { None };
        let out_of_stock = bool::decode(r)?;
        let mut o = Offer {
            cost_a,
            result,
            cost_b,
            out_of_stock,
            uses: i32::decode(r)?,
            max_uses: i32::decode(r)?,
            xp: i32::decode(r)?,
            special_diff: i32::decode(r)?,
            price_multiplier: f32::decode(r)?,
            demand: i32::decode(r)?,
        };
        if o.out_of_stock {
            o.uses = o.max_uses;
        }
        offers.push(o);
    }
    Ok(MerchantOffers {
        window,
        offers,
        level: read_varint(r)?,
        xp: read_varint(r)?,
        show_progress: bool::decode(r)?,
        can_restock: bool::decode(r)?,
    })
}

/// Network item set (`HolderSet`): tag or list of items.
#[derive(Debug, Clone, PartialEq)]
pub enum Holders {
    Tag(String),
    Items(Vec<i32>),
}

fn read_holders(r: &mut Reader<'_>) -> Result<Holders> {
    let n = read_varint(r)?;
    if n == 0 {
        return Ok(Holders::Tag(rc_proto::read_str(r, 32767)?.to_owned()));
    }
    let n = usize::try_from(n - 1).map_err(|_| rc_proto::Error::InvalidValue { what: "set", value: n.into() })?;
    if n > r.remaining() {
        return Err(rc_proto::Error::InvalidValue { what: "set", value: n as i64 });
    }
    (0..n).map(|_| read_varint(r)).collect::<Result<_>>().map(Holders::Items)
}

/// Display of a recipe slot (`slot_display`) reduced to its first stack, if it is one.
fn read_slot_display(items: &ItemCodec, r: &mut Reader<'_>, depth: u32) -> Result<Option<ItemStack>> {
    if depth > 16 {
        return Err(rc_proto::Error::InvalidValue { what: "display nested too deeply", value: depth.into() });
    }
    // Order of the `slot_display` registry (26.3).
    Ok(match read_varint(r)? {
        0 | 1 => None,
        2 => read_slot_display(items, r, depth + 1)?,
        3 => {
            let s = read_slot_display(items, r, depth + 1)?;
            read_varint(r)?;
            s
        }
        4 => Some(ItemStack { item: read_varint(r)?, count: 1, added: Vec::new(), removed: Vec::new() }),
        5 => Some(items.read_template(r)?),
        6 => {
            read_holders(r)?;
            None
        }
        7 => {
            read_slot_display(items, r, depth + 1)?;
            read_slot_display(items, r, depth + 1)?
        }
        8 => {
            let base = read_slot_display(items, r, depth + 1)?;
            read_slot_display(items, r, depth + 1)?;
            if read_varint(r)? == 0 {
                rc_proto::read_str(r, 32767)?;
                rc_proto::nbt::read(r)?;
                bool::decode(r)?;
            }
            base
        }
        9 => {
            let s = read_slot_display(items, r, depth + 1)?;
            read_slot_display(items, r, depth + 1)?;
            s
        }
        10 => {
            let n = rc_proto::read_count(r, 1)?;
            let mut first = None;
            for _ in 0..n {
                let s = read_slot_display(items, r, depth + 1)?;
                first = first.or(s);
            }
            first
        }
        v => return Err(rc_proto::Error::InvalidValue { what: "slot display type", value: v.into() }),
    })
}

/// `update_recipes`: named item sets (smithing, furnaces…) and stonecutter recipes
/// (ingredient, result display).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recipes {
    pub sets: Vec<(String, Vec<i32>)>,
    pub stonecutter: Vec<(Holders, Option<ItemStack>)>,
}

pub fn read_update_recipes(items: &ItemCodec, r: &mut Reader<'_>) -> Result<Recipes> {
    let n = rc_proto::read_count(r, 2)?;
    let mut sets = Vec::with_capacity(n);
    for _ in 0..n {
        let key = rc_proto::read_str(r, 32767)?.to_owned();
        let m = rc_proto::read_count(r, 1)?;
        let ids = (0..m).map(|_| read_varint(r)).collect::<Result<_>>()?;
        sets.push((key, ids));
    }
    let n = rc_proto::read_count(r, 2)?;
    let mut stonecutter = Vec::with_capacity(n);
    for _ in 0..n {
        let input = read_holders(r)?;
        let shown = read_slot_display(items, r, 0)?;
        stonecutter.push((input, shown));
    }
    Ok(Recipes { sets, stonecutter })
}

/// `mount_screen_open`: window, inventory columns, entity.
pub fn read_mount_screen_open(r: &mut Reader<'_>) -> Result<(i32, i32, i32)> {
    Ok((read_varint(r)?, read_varint(r)?, i32::decode(r)?))
}

/// `open_sign_editor`: position, face (true = front).
pub fn read_open_sign_editor(r: &mut Reader<'_>) -> Result<(BlockPos, bool)> {
    Ok((BlockPos::decode(r)?, read_varint(r)? == 1))
}

fn opt_varint(w: &mut Vec<u8>, v: Option<i32>) {
    v.is_some().encode(w);
    if let Some(v) = v {
        write_varint(w, v);
    }
}

/// `select_trade`: offer index.
pub fn write_select_trade(w: &mut Vec<u8>, index: i32) {
    write_varint(w, index);
}

/// `container_button_click`: window, button.
pub fn write_button_click(w: &mut Vec<u8>, window: i32, button: i32) {
    write_varint(w, window);
    write_varint(w, button);
}

/// `container_slot_state_changed`: slot, window, enabled.
pub fn write_slot_state(w: &mut Vec<u8>, slot: i32, window: i32, active: bool) {
    write_varint(w, slot);
    write_varint(w, window);
    active.encode(w);
}

/// `rename_item`: new name.
pub fn write_rename_item(w: &mut Vec<u8>, name: &str) {
    write_str(w, name);
}

/// `set_beacon`: primary and secondary effects (`mob_effect` identifiers).
pub fn write_set_beacon(w: &mut Vec<u8>, primary: Option<i32>, secondary: Option<i32>) {
    opt_varint(w, primary);
    opt_varint(w, secondary);
}

/// `edit_book`: slot, pages (≤ 100), title (signature).
pub fn write_edit_book(w: &mut Vec<u8>, slot: i32, pages: &[String], title: Option<&str>) {
    write_varint(w, slot);
    let n = pages.len().min(100);
    write_varint(w, n as i32);
    for p in &pages[..n] {
        write_str(w, p);
    }
    title.is_some().encode(w);
    if let Some(t) = title {
        write_str(w, t);
    }
}

/// `sign_update`: position, 4 lines, then face (1 = front).
pub fn write_sign_update(w: &mut Vec<u8>, pos: BlockPos, lines: &[String; 4], front: bool) {
    pos.encode(w);
    for l in lines {
        write_str(w, l);
    }
    write_varint(w, i32::from(front));
}

/// `set_command_block`: position, command, mode, options (1 track, 2 conditional, 4 active).
pub fn write_set_command_block(w: &mut Vec<u8>, pos: BlockPos, command: &str, mode: i32, flags: u8) {
    pos.encode(w);
    write_str(w, command);
    write_varint(w, mode);
    w.push(flags);
}

/// `set_command_minecart`: entity, command, track output.
pub fn write_set_command_minecart(w: &mut Vec<u8>, entity: i32, command: &str, track: bool) {
    write_varint(w, entity);
    write_str(w, command);
    track.encode(w);
}

/// Settings of a structure block (§ 6).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StructureBlock {
    pub action: i32,
    pub mode: i32,
    pub name: String,
    pub offset: [i8; 3],
    pub size: [i8; 3],
    pub mirror: i32,
    pub rotation: i32,
    pub data: String,
    pub integrity: f32,
    pub seed: i64,
    pub flags: u8,
}

pub fn write_set_structure_block(w: &mut Vec<u8>, pos: BlockPos, s: &StructureBlock) {
    pos.encode(w);
    write_varint(w, s.action);
    write_varint(w, s.mode);
    write_str(w, &s.name);
    w.extend(s.offset.iter().map(|&v| v as u8));
    w.extend(s.size.iter().map(|&v| v as u8));
    write_varint(w, s.mirror);
    write_varint(w, s.rotation);
    write_str(w, &s.data);
    s.integrity.encode(w);
    write_varlong(w, s.seed);
    w.push(s.flags);
}

/// `set_jigsaw_block`: position, name, target, pool, final state, joint, priorities.
#[allow(clippy::too_many_arguments)]
pub fn write_set_jigsaw_block(
    w: &mut Vec<u8>,
    pos: BlockPos,
    name: &str,
    target: &str,
    pool: &str,
    final_state: &str,
    joint: &str,
    selection_priority: i32,
    placement_priority: i32,
) {
    pos.encode(w);
    for s in [name, target, pool, final_state, joint] {
        write_str(w, s);
    }
    write_varint(w, selection_priority);
    write_varint(w, placement_priority);
}

/// `jigsaw_generate`: position, levels, keep jigsaws.
pub fn write_jigsaw_generate(w: &mut Vec<u8>, pos: BlockPos, levels: i32, keep: bool) {
    pos.encode(w);
    write_varint(w, levels);
    keep.encode(w);
}

/// `set_test_block`: position, mode, message.
pub fn write_set_test_block(w: &mut Vec<u8>, pos: BlockPos, mode: i32, message: &str) {
    pos.encode(w);
    write_varint(w, mode);
    write_str(w, message);
}

/// `test_instance_block_action`: position, action (0 init … 6 run), test (optional
/// identifier), size, rotation, ignored entities, state (always "cleared"), without message.
pub fn write_test_instance_action(
    w: &mut Vec<u8>,
    pos: BlockPos,
    action: i32,
    test: Option<&str>,
    size: [i32; 3],
    rotation: i32,
    ignore_entities: bool,
) {
    pos.encode(w);
    write_varint(w, action);
    test.is_some().encode(w);
    if let Some(t) = test {
        write_str(w, t);
    }
    for v in size {
        write_varint(w, v);
    }
    write_varint(w, rotation);
    ignore_entities.encode(w);
    write_varint(w, 0);
    false.encode(w);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_update_lines_before_face() {
        let mut w = Vec::new();
        let lines = ["a".to_owned(), String::new(), "bc".into(), String::new()];
        write_sign_update(&mut w, BlockPos::new(1, 2, 3), &lines, true);
        assert_eq!(&w[8..], &[1, b'a', 0, 2, b'b', b'c', 0, 1]);
    }

    #[test]
    fn beacon_optional_effects() {
        let mut w = Vec::new();
        write_set_beacon(&mut w, Some(1), None);
        assert_eq!(w, [1, 1, 0]);
    }
}
