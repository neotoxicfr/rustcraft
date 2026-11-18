//! Recipe book packets (`docs/specs/creative-inventory.md` § 9): slot and recipe
//! displays, book entries, settings, ghost recipe.

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Reader, Result};
use rc_proto_v777::item::{ItemCodec, ItemStack};

/// Set of item references: named tag or list of identifiers.
#[derive(Debug, Clone, PartialEq)]
pub enum HolderSet {
    Tag(String),
    Ids(Vec<i32>),
}

/// Slot display (`slot_display` registry, § 9.4).
#[derive(Debug, Clone, PartialEq)]
pub enum SlotDisplay {
    Empty,
    AnyFuel,
    WithAnyPotion(Box<SlotDisplay>),
    OnlyWithComponent(Box<SlotDisplay>, i32),
    Item(i32),
    Stack(ItemStack),
    Tag(HolderSet),
    Dyed(Box<SlotDisplay>, Box<SlotDisplay>),
    SmithingTrim(Box<SlotDisplay>, Box<SlotDisplay>),
    WithRemainder(Box<SlotDisplay>),
    Composite(Vec<SlotDisplay>),
}

/// Recipe display (`recipe_display` registry, § 9.3).
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeDisplay {
    Shapeless {
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        station: SlotDisplay,
    },
    Shaped {
        width: i32,
        height: i32,
        ingredients: Vec<SlotDisplay>,
        result: SlotDisplay,
        station: SlotDisplay,
    },
    Furnace {
        ingredient: SlotDisplay,
        fuel: SlotDisplay,
        result: SlotDisplay,
        station: SlotDisplay,
        time: i32,
        xp: f32,
    },
    Stonecutter {
        input: SlotDisplay,
        result: SlotDisplay,
        station: SlotDisplay,
    },
    Smithing {
        template: SlotDisplay,
        base: SlotDisplay,
        addition: SlotDisplay,
        result: SlotDisplay,
        station: SlotDisplay,
    },
}

impl RecipeDisplay {
    pub fn result(&self) -> &SlotDisplay {
        match self {
            RecipeDisplay::Shapeless { result, .. }
            | RecipeDisplay::Shaped { result, .. }
            | RecipeDisplay::Furnace { result, .. }
            | RecipeDisplay::Stonecutter { result, .. }
            | RecipeDisplay::Smithing { result, .. } => result,
        }
    }

    pub fn station(&self) -> &SlotDisplay {
        match self {
            RecipeDisplay::Shapeless { station, .. }
            | RecipeDisplay::Shaped { station, .. }
            | RecipeDisplay::Furnace { station, .. }
            | RecipeDisplay::Stonecutter { station, .. }
            | RecipeDisplay::Smithing { station, .. } => station,
        }
    }
}

/// Book entry (§ 9.2).
#[derive(Debug, Clone, PartialEq)]
pub struct RecipeEntry {
    pub id: i32,
    pub display: RecipeDisplay,
    pub group: Option<i32>,
    pub category: i32,
    pub requirements: Option<Vec<HolderSet>>,
}

/// Clientbound packets of the book.
#[derive(Debug, Clone, PartialEq)]
pub enum RecipeEvent {
    /// Entries and flags (bit 0 notification, bit 1 highlight), then "replace".
    Add(Vec<(RecipeEntry, u8)>, bool),
    Remove(Vec<i32>),
    /// (open, filter): crafting table, furnace, blast furnace, smoker.
    Settings([(bool, bool); 4]),
    Ghost {
        window: i32,
        display: Box<RecipeDisplay>,
    },
}

const MAX_DEPTH: u32 = 16;

fn count(r: &mut Reader<'_>) -> Result<usize> {
    rc_proto::read_count(r, 1)
}

fn holder_set(r: &mut Reader<'_>) -> Result<HolderSet> {
    let n = read_varint(r)?;
    if n == 0 {
        return Ok(HolderSet::Tag(rc_proto::read_str(r, 32767)?.to_owned()));
    }
    let n = usize::try_from(n - 1)
        .map_err(|_| rc_proto::Error::InvalidValue { what: "reference set", value: 0 })?;
    if n > r.rest().len() {
        return Err(rc_proto::Error::InvalidValue { what: "reference set", value: 0 });
    }
    (0..n).map(|_| read_varint(r)).collect::<Result<_>>().map(HolderSet::Ids)
}

fn slot(r: &mut Reader<'_>, items: &ItemCodec, depth: u32) -> Result<SlotDisplay> {
    if depth > MAX_DEPTH {
        return Err(rc_proto::Error::InvalidValue { what: "slot display nested too deeply", value: 0 });
    }
    let sub = |r: &mut Reader<'_>| slot(r, items, depth + 1).map(Box::new);
    Ok(match read_varint(r)? {
        0 => SlotDisplay::Empty,
        1 => SlotDisplay::AnyFuel,
        2 => SlotDisplay::WithAnyPotion(sub(r)?),
        3 => {
            let s = sub(r)?;
            SlotDisplay::OnlyWithComponent(s, read_varint(r)?)
        }
        4 => SlotDisplay::Item(read_varint(r)?),
        5 => SlotDisplay::Stack(items.read_template(r)?),
        6 => SlotDisplay::Tag(holder_set(r)?),
        7 => {
            let d = sub(r)?;
            SlotDisplay::Dyed(d, sub(r)?)
        }
        8 => {
            let base = sub(r)?;
            let material = sub(r)?;
            // Pattern: reference (identifier + 1) or inline value (active, description, decal).
            if read_varint(r)? == 0 {
                rc_proto::read_str(r, 32767)?;
                rc_proto::nbt::skip(r)?;
                bool::decode(r)?;
            }
            SlotDisplay::SmithingTrim(base, material)
        }
        9 => {
            let input = sub(r)?;
            sub(r)?;
            SlotDisplay::WithRemainder(input)
        }
        10 => {
            let n = count(r)?;
            SlotDisplay::Composite((0..n).map(|_| slot(r, items, depth + 1)).collect::<Result<_>>()?)
        }
        _ => return Err(rc_proto::Error::InvalidValue { what: "slot display type", value: 0 }),
    })
}

fn slots(r: &mut Reader<'_>, items: &ItemCodec) -> Result<Vec<SlotDisplay>> {
    let n = count(r)?;
    (0..n).map(|_| slot(r, items, 0)).collect()
}

fn display(r: &mut Reader<'_>, items: &ItemCodec) -> Result<RecipeDisplay> {
    let s = |r: &mut Reader<'_>| slot(r, items, 0);
    Ok(match read_varint(r)? {
        0 => {
            let ingredients = slots(r, items)?;
            RecipeDisplay::Shapeless { ingredients, result: s(r)?, station: s(r)? }
        }
        1 => {
            let (width, height) = (read_varint(r)?, read_varint(r)?);
            let ingredients = slots(r, items)?;
            RecipeDisplay::Shaped { width, height, ingredients, result: s(r)?, station: s(r)? }
        }
        2 => {
            let (ingredient, fuel, result, station) = (s(r)?, s(r)?, s(r)?, s(r)?);
            RecipeDisplay::Furnace { ingredient, fuel, result, station, time: read_varint(r)?, xp: f32::decode(r)? }
        }
        3 => {
            let (input, result) = (s(r)?, s(r)?);
            RecipeDisplay::Stonecutter { input, result, station: s(r)? }
        }
        4 => {
            let (template, base, addition, result) = (s(r)?, s(r)?, s(r)?, s(r)?);
            RecipeDisplay::Smithing { template, base, addition, result, station: s(r)? }
        }
        _ => return Err(rc_proto::Error::InvalidValue { what: "recipe display type", value: 0 }),
    })
}

fn entry(r: &mut Reader<'_>, items: &ItemCodec) -> Result<RecipeEntry> {
    let id = read_varint(r)?;
    let display = display(r, items)?;
    let group = match read_varint(r)? {
        0 => None,
        g => Some(g - 1),
    };
    let category = read_varint(r)?;
    let requirements = if bool::decode(r)? {
        let n = count(r)?;
        Some((0..n).map(|_| holder_set(r)).collect::<Result<_>>()?)
    } else {
        None
    };
    Ok(RecipeEntry { id, display, group, category, requirements })
}

pub fn read_add(r: &mut Reader<'_>, items: &ItemCodec) -> Result<RecipeEvent> {
    let n = count(r)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let e = entry(r, items)?;
        out.push((e, u8::decode(r)?));
    }
    Ok(RecipeEvent::Add(out, bool::decode(r)?))
}

pub fn read_remove(r: &mut Reader<'_>) -> Result<RecipeEvent> {
    let n = count(r)?;
    (0..n).map(|_| read_varint(r)).collect::<Result<_>>().map(RecipeEvent::Remove)
}

pub fn read_settings(r: &mut Reader<'_>) -> Result<RecipeEvent> {
    let mut s = [(false, false); 4];
    for x in &mut s {
        *x = (bool::decode(r)?, bool::decode(r)?);
    }
    Ok(RecipeEvent::Settings(s))
}

pub fn read_ghost(r: &mut Reader<'_>, items: &ItemCodec) -> Result<RecipeEvent> {
    let window = read_varint(r)?;
    Ok(RecipeEvent::Ghost { window, display: Box::new(display(r, items)?) })
}
