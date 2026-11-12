//! Typed keys for the block-state properties, and the index that reads and writes them without
//! comparing a name, formatting a number or parsing a value.
//!
//! The code names a property with a [`PropKey`]. [`PropIndex`], built once from the extracted
//! data, lists the properties of each block by key and the facts carried by each list of values
//! (integers, booleans, directions, axes), so a read is a handful of table loads instead of a
//! string search. The hottest properties are also packed per state ([`PropIndex::packed_bool`]),
//! one load per read. [`ValueFacts`] is shared with the decoration's own block table.

use crate::VersionData;

/// Marks an absent entry (value index, key, direction) in the tables of this module.
pub const NONE: u8 = u8::MAX;

/// Direction names in the game's value order: down, up, north, south, west, east.
pub const DIRECTION_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];

/// Axis names, in order x, y, z.
pub const AXIS_NAMES: [&str; 3] = ["x", "y", "z"];

macro_rules! prop_keys {
    ($($key:ident = $name:literal,)*) => {
        /// A block-state property named by the code, resolved against the data at run time.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum PropKey {
            $($key,)*
        }

        impl PropKey {
            /// Every key, in declaration order (`key as usize` is its rank).
            pub const ALL: &'static [PropKey] = &[$(PropKey::$key,)*];

            /// Name of the property in the game data.
            pub const fn name(self) -> &'static str {
                match self {
                    $(PropKey::$key => $name,)*
                }
            }

            /// Key of a property name, `None` for a name the code never asks for.
            pub fn from_name(name: &str) -> Option<PropKey> {
                match name {
                    $($name => Some(PropKey::$key),)*
                    _ => None,
                }
            }
        }
    };
}

prop_keys! {
    Age = "age",
    Attached = "attached",
    Attachment = "attachment",
    Axis = "axis",
    Berries = "berries",
    Bites = "bites",
    Bottom = "bottom",
    CanSummon = "can_summon",
    Charges = "charges",
    Conditional = "conditional",
    CopperGolemPose = "copper_golem_pose",
    Cracked = "cracked",
    Crafting = "crafting",
    CreakingHeartState = "creaking_heart_state",
    Delay = "delay",
    Disarmed = "disarmed",
    Distance = "distance",
    Down = "down",
    Dusted = "dusted",
    East = "east",
    Eggs = "eggs",
    Enabled = "enabled",
    Extended = "extended",
    Eye = "eye",
    Face = "face",
    Facing = "facing",
    FlowerAmount = "flower_amount",
    Half = "half",
    Hanging = "hanging",
    HasBook = "has_book",
    HasBottle0 = "has_bottle_0",
    HasBottle1 = "has_bottle_1",
    HasBottle2 = "has_bottle_2",
    HasRecord = "has_record",
    Hatch = "hatch",
    Hinge = "hinge",
    HoneyLevel = "honey_level",
    Hydration = "hydration",
    InWall = "in_wall",
    Instrument = "instrument",
    Inverted = "inverted",
    Layers = "layers",
    Leaves = "leaves",
    Level = "level",
    Lit = "lit",
    Locked = "locked",
    Mode = "mode",
    Moisture = "moisture",
    Natural = "natural",
    North = "north",
    Note = "note",
    Occupied = "occupied",
    Ominous = "ominous",
    Open = "open",
    Orientation = "orientation",
    Part = "part",
    Persistent = "persistent",
    Pickles = "pickles",
    PotentSulfurState = "potent_sulfur_state",
    Power = "power",
    Powered = "powered",
    Rotation = "rotation",
    SculkSensorPhase = "sculk_sensor_phase",
    SegmentAmount = "segment_amount",
    Shape = "shape",
    Shrieking = "shrieking",
    SideChain = "side_chain",
    SignalFire = "signal_fire",
    Slot0Occupied = "slot_0_occupied",
    Slot1Occupied = "slot_1_occupied",
    Slot2Occupied = "slot_2_occupied",
    Slot3Occupied = "slot_3_occupied",
    Slot4Occupied = "slot_4_occupied",
    Slot5Occupied = "slot_5_occupied",
    Snowy = "snowy",
    South = "south",
    Stage = "stage",
    Thickness = "thickness",
    Tilt = "tilt",
    Tip = "tip",
    TrialSpawnerState = "trial_spawner_state",
    Triggered = "triggered",
    Type = "type",
    Unstable = "unstable",
    Up = "up",
    VaultState = "vault_state",
    VerticalDirection = "vertical_direction",
    Waterlogged = "waterlogged",
    West = "west",
}

impl PropKey {
    /// Key of the side property named like the direction (game order: down, up, north, south,
    /// west, east), as fences, vines and walls name their sides.
    pub const fn side(direction: usize) -> PropKey {
        [PropKey::Down, PropKey::Up, PropKey::North, PropKey::South, PropKey::West, PropKey::East][direction]
    }

    /// Key of the occupied flag of shelf or bookshelf slot `slot` (0..=5), `None` beyond.
    pub const fn slot_occupied(slot: usize) -> Option<PropKey> {
        const SLOTS: [PropKey; 6] = [
            PropKey::Slot0Occupied,
            PropKey::Slot1Occupied,
            PropKey::Slot2Occupied,
            PropKey::Slot3Occupied,
            PropKey::Slot4Occupied,
            PropKey::Slot5Occupied,
        ];
        if slot < SLOTS.len() { Some(SLOTS[slot]) } else { None }
    }

    /// Key of the bottle flag of brewing stand slot `slot` (0..=2), `None` beyond.
    pub const fn has_bottle(slot: usize) -> Option<PropKey> {
        const SLOTS: [PropKey; 3] = [PropKey::HasBottle0, PropKey::HasBottle1, PropKey::HasBottle2];
        if slot < SLOTS.len() { Some(SLOTS[slot]) } else { None }
    }

    /// Field of the per-state packed facts that holds this property, `None` if not packed.
    ///
    /// The hottest reads of the server (redstone, fluids, leaves, crops) get one load per read;
    /// a `match` so that a constant key folds the lookup away at compile time.
    #[inline]
    pub const fn packed_field(self) -> Option<PackedField> {
        use PackedKind::{Bool, Direction, Int};
        let (shift, width, kind) = match self {
            PropKey::Facing => (0, 3, Direction),
            PropKey::Power => (3, 4, Int),
            PropKey::Level => (7, 4, Int),
            PropKey::Distance => (11, 4, Int),
            PropKey::Age => (15, 5, Int),
            PropKey::Powered => (20, 1, Bool),
            PropKey::Lit => (21, 1, Bool),
            PropKey::Extended => (22, 1, Bool),
            PropKey::Triggered => (23, 1, Bool),
            PropKey::Locked => (24, 1, Bool),
            PropKey::Waterlogged => (25, 1, Bool),
            PropKey::Open => (26, 1, Bool),
            PropKey::Attached => (27, 1, Bool),
            PropKey::Hanging => (28, 1, Bool),
            PropKey::Persistent => (29, 1, Bool),
            PropKey::Conditional => (30, 1, Bool),
            PropKey::Snowy => (31, 1, Bool),
            _ => return None,
        };
        Some(PackedField { shift, width, kind })
    }
}

/// How a packed field reads its property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedKind {
    /// 1 bit: the value is `true`; absent reads `false`.
    Bool,
    /// Small non-negative integer; absent or not a number reads 0.
    Int,
    /// Direction in game order; 7 = absent or not a direction.
    Direction,
}

/// Bits of one property in the per-state packed facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedField {
    pub shift: u8,
    pub width: u8,
    pub kind: PackedKind,
}

impl PackedField {
    const fn mask(self) -> u32 {
        (1 << self.width) - 1
    }
}

/// Direction field value meaning "absent or not a direction".
const PACKED_NO_DIRECTION: u32 = 7;

/// Facts of a list of property values, resolved once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueFacts {
    /// First value when every value parses as the integer one above the previous one.
    pub int_first: Option<i32>,
    /// Each value is written exactly as its integer prints (`"7"`, never `"07"`): an integer
    /// write can then find its value by subtraction instead of formatting it.
    pub canonical_ints: bool,
    /// Indices of `false` and `true` ([`NONE`] if absent).
    pub bool_index: [u8; 2],
    /// Index of each direction name, game order ([`NONE`] if absent).
    pub dir_index: [u8; 6],
    /// Index of `x`, `y`, `z` ([`NONE`] if absent).
    pub axis_index: [u8; 3],
}

impl ValueFacts {
    pub fn of(values: &[String]) -> ValueFacts {
        let position = |name: &str| values.iter().position(|x| x == name).map_or(NONE, |i| i as u8);
        let int_first = values.first().and_then(|v| v.parse::<i32>().ok()).filter(|&first| {
            values.iter().enumerate().all(|(i, x)| x.parse::<i32>().ok() == first.checked_add(i as i32))
        });
        let canonical_ints = values.iter().all(|x| x.parse::<i32>().is_ok_and(|n| n.to_string() == *x));
        ValueFacts {
            int_first,
            canonical_ints,
            bool_index: [position("false"), position("true")],
            dir_index: DIRECTION_NAMES.map(position),
            axis_index: AXIS_NAMES.map(position),
        }
    }
}

/// One value of a property, as the typed reads see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ValueInfo {
    /// Integer read (0 if the value is not a number, like a failed parse).
    int: i32,
    /// The value is exactly the decimal form of `int` (an integer write can land on it).
    canonical_int: bool,
    /// Direction index in game order, [`NONE`] if not a direction name.
    dir: u8,
}

/// One property of a block, as found by key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PropSlot {
    /// `PropKey as u8`, or [`NONE`] for a property the code never names.
    pub key: u8,
    /// Rank among the block's properties (offset in `VersionData::state_values`).
    pub position: u8,
    /// Number of values.
    pub count: u8,
    /// Index into `VersionData::properties`.
    pub property: u16,
    /// Start of this property's values in the index's value table.
    value_start: u32,
    /// States between two consecutive values of this property.
    pub stride: u32,
}

/// Property index of every block and state of the data.
#[derive(Debug, Clone)]
pub struct PropIndex {
    /// Per block: first slot in `slots` and how many.
    block_slots: Vec<(u32, u8)>,
    slots: Vec<PropSlot>,
    /// Per property of `VersionData::properties`.
    facts: Vec<ValueFacts>,
    /// Every value of every property, from `PropSlot::value_start`.
    values: Vec<ValueInfo>,
    /// Per state: the packed fields of [`PropKey::packed_field`].
    packed: Vec<u32>,
    /// Bit `shift` set: that packed field holds the exact read for every state (an integer
    /// above its width leaves the field unused, reads then go through the slots).
    usable_fields: u32,
    /// Test hook: a missing value keeps the current one instead of the default (to prove that
    /// the equivalence check of the property change covers the default rule).
    default_rule_sabotaged: bool,
}

impl PropIndex {
    pub fn new(d: &VersionData) -> PropIndex {
        let mut value_starts = Vec::with_capacity(d.properties.len());
        let mut values = Vec::new();
        for p in &d.properties {
            value_starts.push(values.len() as u32);
            values.extend(p.values.iter().map(|v| {
                let parsed = v.parse::<i32>().ok();
                ValueInfo {
                    int: parsed.unwrap_or(0),
                    canonical_int: parsed.is_some_and(|n| n.to_string() == *v),
                    dir: DIRECTION_NAMES.iter().position(|x| x == v).map_or(NONE, |i| i as u8),
                }
            }));
        }
        let mut block_slots = Vec::with_capacity(d.blocks.len());
        let mut slots = Vec::new();
        for b in &d.blocks {
            let counts: Vec<u32> = b.properties.iter().map(|&p| d.properties[p as usize].values.len() as u32).collect();
            block_slots.push((slots.len() as u32, b.properties.len() as u8));
            for (i, &p) in b.properties.iter().enumerate() {
                slots.push(PropSlot {
                    key: PropKey::from_name(&d.properties[p as usize].name).map_or(NONE, |k| k as u8),
                    position: i as u8,
                    count: counts[i] as u8,
                    property: p,
                    value_start: value_starts[p as usize],
                    stride: counts[i + 1..].iter().product(),
                });
            }
        }
        let facts = d.properties.iter().map(|p| ValueFacts::of(&p.values)).collect();
        let mut index =
            PropIndex { block_slots, slots, facts, values, packed: Vec::new(), usable_fields: 0, default_rule_sabotaged: false };
        index.pack_states(d);
        index
    }

    /// Fills the packed facts of every state from the slots; a field whose integer does not
    /// fit its width for some state stays unused.
    fn pack_states(&mut self, d: &VersionData) {
        let fields: Vec<(PropKey, PackedField)> =
            PropKey::ALL.iter().filter_map(|&k| k.packed_field().map(|f| (k, f))).collect();
        let mut usable = fields.iter().fold(0u32, |m, (_, f)| m | 1 << f.shift);
        let mut packed = Vec::with_capacity(d.states.len());
        for s in 0..d.states.len() as u32 {
            let mut word = 0u32;
            for &(key, field) in &fields {
                let raw = match field.kind {
                    PackedKind::Bool => u32::from(self.get_bool(d, s, key)),
                    PackedKind::Direction => self.get_dir(d, s, key).map_or(PACKED_NO_DIRECTION, u32::from),
                    PackedKind::Int => {
                        let n = self.get_int(d, s, key);
                        match u32::try_from(n) {
                            Ok(n) if n <= field.mask() => n,
                            _ => {
                                usable &= !(1 << field.shift);
                                0
                            }
                        }
                    }
                };
                word |= raw << field.shift;
            }
            packed.push(word);
        }
        self.packed = packed;
        self.usable_fields = usable;
    }

    /// Properties of `block`, in the data's order (by increasing name).
    #[inline]
    pub fn block_slots(&self, block: u16) -> &[PropSlot] {
        match self.block_slots.get(block as usize) {
            Some(&(start, len)) => &self.slots[start as usize..start as usize + len as usize],
            None => &[],
        }
    }

    /// Facts of the values of property `property` (index into `VersionData::properties`).
    #[inline]
    pub fn value_facts(&self, property: u16) -> &ValueFacts {
        &self.facts[property as usize]
    }

    #[inline]
    fn slot_of(&self, d: &VersionData, s: u32, key: PropKey) -> Option<&PropSlot> {
        let info = d.states.get(s as usize)?;
        self.block_slots(info.block).iter().find(|p| p.key == key as u8)
    }

    /// Slot of `key` in the block of `s` and the current value index.
    #[inline]
    fn find(&self, d: &VersionData, s: u32, key: PropKey) -> Option<(&PropSlot, u8)> {
        let info = d.states.get(s as usize)?;
        let slot = self.block_slots(info.block).iter().find(|p| p.key == key as u8)?;
        Some((slot, d.state_values[info.values as usize + slot.position as usize]))
    }

    #[inline]
    fn value(&self, slot: &PropSlot, index: u8) -> ValueInfo {
        self.values[slot.value_start as usize + index as usize]
    }

    /// State `s` with value index `new` instead of `current` for `slot`.
    #[inline]
    fn moved(s: u32, slot: &PropSlot, current: u8, new: u8) -> u32 {
        // The states of a block are the cartesian product of its property values: one value
        // step of a property is `stride` states.
        s - u32::from(current) * slot.stride + u32::from(new) * slot.stride
    }

    #[inline]
    pub fn has(&self, d: &VersionData, s: u32, key: PropKey) -> bool {
        self.slot_of(d, s, key).is_some()
    }

    /// Text value of `key` in state `s`.
    #[inline]
    pub fn get<'d>(&self, d: &'d VersionData, s: u32, key: PropKey) -> Option<&'d str> {
        let (slot, v) = self.find(d, s, key)?;
        Some(d.properties[slot.property as usize].values[v as usize].as_str())
    }

    /// The value of `key` is `true` (absent: `false`).
    #[inline]
    pub fn get_bool(&self, d: &VersionData, s: u32, key: PropKey) -> bool {
        self.find(d, s, key).is_some_and(|(slot, v)| v == self.facts[slot.property as usize].bool_index[1])
    }

    /// Integer value of `key` (absent or not a number: 0).
    #[inline]
    pub fn get_int(&self, d: &VersionData, s: u32, key: PropKey) -> i32 {
        self.find(d, s, key).map_or(0, |(slot, v)| self.value(slot, v).int)
    }

    /// Direction of `key` in game order (absent or not a direction: `None`).
    #[inline]
    pub fn get_dir(&self, d: &VersionData, s: u32, key: PropKey) -> Option<u8> {
        let (slot, v) = self.find(d, s, key)?;
        let dir = self.value(slot, v).dir;
        (dir != NONE).then_some(dir)
    }

    /// State with `key` at value index `index` of its value list (unchanged if absent or out
    /// of range, [`NONE`] included).
    #[inline]
    fn with_index(&self, d: &VersionData, s: u32, key: PropKey, index: impl Fn(&ValueFacts) -> u8) -> u32 {
        let Some((slot, v)) = self.find(d, s, key) else { return s };
        let new = index(&self.facts[slot.property as usize]);
        if new < slot.count { Self::moved(s, slot, v, new) } else { s }
    }

    /// State with `key = value` (unchanged if the property or the value does not exist).
    #[inline]
    pub fn set(&self, d: &VersionData, s: u32, key: PropKey, value: &str) -> u32 {
        let Some((slot, v)) = self.find(d, s, key) else { return s };
        match d.properties[slot.property as usize].values.iter().position(|x| x == value) {
            Some(new) => Self::moved(s, slot, v, new as u8),
            None => s,
        }
    }

    #[inline]
    pub fn set_bool(&self, d: &VersionData, s: u32, key: PropKey, value: bool) -> u32 {
        self.with_index(d, s, key, |f| f.bool_index[usize::from(value)])
    }

    /// State with the direction `dir` (game order) for `key`.
    #[inline]
    pub fn set_dir(&self, d: &VersionData, s: u32, key: PropKey, dir: u8) -> u32 {
        self.with_index(d, s, key, |f| f.dir_index.get(usize::from(dir)).copied().unwrap_or(NONE))
    }

    /// State with the integer `value` for `key`: the value written exactly as `value` prints
    /// (unchanged if there is none).
    #[inline]
    pub fn set_int(&self, d: &VersionData, s: u32, key: PropKey, value: i32) -> u32 {
        let Some((slot, v)) = self.find(d, s, key) else { return s };
        match self.int_position(slot, value) {
            Some(new) => Self::moved(s, slot, v, new),
            None => s,
        }
    }

    /// Value index of the property of `slot` in the default state of `block`.
    #[inline]
    fn default_index(d: &VersionData, block: u16, slot: &PropSlot) -> u8 {
        let default = d.blocks[block as usize].default_state;
        d.state_values[d.states[default as usize].values as usize + slot.position as usize]
    }

    /// State with `key = value` as a block state rebuilt from text gives it: a value the
    /// property lacks becomes the block's default value of that property, a property the block
    /// lacks leaves the state as is; `None` for an unknown state.
    #[inline]
    pub fn set_or_default(&self, d: &VersionData, s: u32, key: PropKey, value: &str) -> Option<u32> {
        self.set_found_or_default(d, s, key, |slot| {
            d.properties[slot.property as usize].values.iter().position(|x| x == value).map(|i| i as u8)
        })
    }

    /// [`Self::set_or_default`] with the value written as the integer `value` prints.
    #[inline]
    pub fn set_int_or_default(&self, d: &VersionData, s: u32, key: PropKey, value: i32) -> Option<u32> {
        self.set_found_or_default(d, s, key, |slot| self.int_position(slot, value))
    }

    #[inline]
    fn set_found_or_default(
        &self,
        d: &VersionData,
        s: u32,
        key: PropKey,
        position: impl FnOnce(&PropSlot) -> Option<u8>,
    ) -> Option<u32> {
        let info = d.states.get(s as usize)?;
        let Some(slot) = self.block_slots(info.block).iter().find(|p| p.key == key as u8) else { return Some(s) };
        let current = d.state_values[info.values as usize + slot.position as usize];
        let new = position(slot).unwrap_or_else(|| {
            if self.default_rule_sabotaged { current } else { Self::default_index(d, info.block, slot) }
        });
        Some(Self::moved(s, slot, current, new))
    }

    /// Index of the value written exactly as `value` prints, if any.
    #[inline]
    fn int_position(&self, slot: &PropSlot, value: i32) -> Option<u8> {
        let facts = &self.facts[slot.property as usize];
        match facts.int_first {
            // Consecutive integers in game order: the index is a subtraction.
            Some(first) if facts.canonical_ints => {
                let i = i64::from(value) - i64::from(first);
                (0..i64::from(slot.count)).contains(&i).then_some(i as u8)
            }
            _ => (0..slot.count).find(|&i| {
                let x = self.value(slot, i);
                x.canonical_int && x.int == value
            }),
        }
    }

    /// Next value of `key` (cycle), unchanged if absent.
    #[inline]
    pub fn cycle(&self, d: &VersionData, s: u32, key: PropKey) -> u32 {
        let Some((slot, v)) = self.find(d, s, key) else { return s };
        Self::moved(s, slot, v, ((u32::from(v) + 1) % u32::from(slot.count)) as u8)
    }

    /// Packed field of `key` for state `s` when it holds the exact read of that kind.
    #[inline]
    fn packed_raw(&self, s: u32, key: PropKey, kind: PackedKind) -> Option<u32> {
        let f = key.packed_field()?;
        if f.kind != kind || self.usable_fields & 1 << f.shift == 0 {
            return None;
        }
        Some(self.packed.get(s as usize)? >> f.shift & f.mask())
    }

    /// Packed boolean of `key` in state `s`, `None` if `key` is not a packed boolean.
    #[inline]
    pub fn packed_bool(&self, s: u32, key: PropKey) -> Option<bool> {
        self.packed_raw(s, key, PackedKind::Bool).map(|b| b != 0)
    }

    /// Packed integer of `key` in state `s`, `None` if `key` is not a usable packed integer.
    #[inline]
    pub fn packed_int(&self, s: u32, key: PropKey) -> Option<i32> {
        self.packed_raw(s, key, PackedKind::Int).map(|n| n as i32)
    }

    /// Packed direction of `key` in state `s` (game order; inner `None`: absent or not a
    /// direction), outer `None` if `key` is not a packed direction.
    #[inline]
    pub fn packed_dir(&self, s: u32, key: PropKey) -> Option<Option<u8>> {
        self.packed_raw(s, key, PackedKind::Direction).map(|d| (d != PACKED_NO_DIRECTION).then_some(d as u8))
    }

    /// `key` is packed and its field holds the exact read for every state of the data.
    pub fn packed_usable(&self, key: PropKey) -> bool {
        key.packed_field().is_some_and(|f| self.usable_fields & 1 << f.shift != 0)
    }

    /// Packed word of state `s` (tests).
    #[doc(hidden)]
    pub fn packed_word(&self, s: u32) -> u32 {
        self.packed[s as usize]
    }

    /// Test hook: overwrites the facts of a property (to prove that the equivalence checks
    /// catch a wrong table).
    #[doc(hidden)]
    pub fn sabotage_value_facts(&mut self, property: u16, change: impl FnOnce(&mut ValueFacts)) {
        change(&mut self.facts[property as usize]);
    }

    /// Test hook: overwrites the packed facts of a state.
    #[doc(hidden)]
    pub fn sabotage_packed(&mut self, s: u32, word: u32) {
        self.packed[s as usize] = word;
    }

    /// Test hook: a property change to a value the property lacks keeps the current value
    /// instead of taking the block's default value.
    #[doc(hidden)]
    pub fn sabotage_default_rule(&mut self) {
        self.default_rule_sabotaged = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn keys_round_trip_their_names() {
        for &k in PropKey::ALL {
            assert_eq!(PropKey::from_name(k.name()), Some(k));
            assert_eq!(PropKey::ALL[k as usize], k);
        }
        assert_eq!(PropKey::from_name("no_such_property"), None);
        assert!(PropKey::ALL.len() < usize::from(NONE), "keys must fit below the absent marker");
        for (i, name) in DIRECTION_NAMES.iter().enumerate() {
            assert_eq!(PropKey::side(i).name(), *name);
        }
        assert_eq!(PropKey::slot_occupied(5).map(PropKey::name), Some("slot_5_occupied"));
        assert_eq!(PropKey::slot_occupied(6), None);
    }

    #[test]
    fn packed_fields_do_not_overlap() {
        let mut used = 0u64;
        for &k in PropKey::ALL {
            if let Some(f) = k.packed_field() {
                let bits = u64::from(f.mask()) << f.shift;
                assert_eq!(used & bits, 0, "{} overlaps another packed field", k.name());
                assert!(u32::from(f.shift) + u32::from(f.width) <= 32, "{} outside the word", k.name());
                used |= bits;
            }
        }
    }

    #[test]
    fn value_facts_of_typical_lists() {
        let ages = ValueFacts::of(&strings(&["0", "1", "2", "3"]));
        assert_eq!((ages.int_first, ages.canonical_ints), (Some(0), true));
        let padded = ValueFacts::of(&strings(&["01", "02"]));
        assert_eq!((padded.int_first, padded.canonical_ints), (Some(1), false));
        let gaps = ValueFacts::of(&strings(&["1", "3"]));
        assert_eq!((gaps.int_first, gaps.canonical_ints), (None, true));
        let flags = ValueFacts::of(&strings(&["true", "false"]));
        assert_eq!(flags.bool_index, [1, 0]);
        assert_eq!(flags.int_first, None);
        let facing = ValueFacts::of(&strings(&["north", "east", "south", "west", "up", "down"]));
        assert_eq!(facing.dir_index, [5, 4, 0, 2, 3, 1]);
        let axis = ValueFacts::of(&strings(&["x", "z"]));
        assert_eq!(axis.axis_index, [0, NONE, 1]);
    }
}
