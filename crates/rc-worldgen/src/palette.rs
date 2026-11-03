//! Block states of generation: table of compact identifiers (`u16`) to their full name
//! (`minecraft:name` for the default state, `minecraft:name[p=v,…]` otherwise), with the properties
//! the terrain needs (air, fluid, tags of the height maps and the caves).

use hashbrown::{HashMap, HashSet};

use crate::data::{DataPack, full_id};
use crate::json::Json;

pub type BlockId = u16;

/// Air always has identifier 0.
pub const AIR: BlockId = 0;

const FLAG_AIR: u8 = 1;
const FLAG_FLUID: u8 = 2;
const FLAG_WATER: u8 = 4;
const FLAG_LAVA: u8 = 8;
const FLAG_MOTION: u8 = 16;
const FLAG_UNCARVABLE: u8 = 32;

/// Table of the states known to a generator (filled at preparation, frozen afterwards).
#[derive(Debug, Clone)]
pub struct BlockTable {
    names: Vec<String>,
    flags: Vec<u8>,
    /// Per state: identifier of its block (name without the properties), numbered in order of appearance.
    base_ids: Vec<u16>,
    index: HashMap<String, BlockId>,
    /// Block name (without the properties) to its base identifier.
    base_index: HashMap<String, u16>,
    motion: HashSet<String>,
    uncarvable: HashSet<String>,
}

/// Block name without the properties.
fn block_of(state: &str) -> &str {
    state.split_once('[').map_or(state, |(b, _)| b)
}

impl BlockTable {
    /// Table with air, the tags `blocks_motion_in_heightmap` and `uncarvable` resolved from
    /// the data (empty if absent: only air and fluids are then distinguished).
    pub fn new(pack: &DataPack) -> Self {
        let mut t = BlockTable {
            names: Vec::new(),
            flags: Vec::new(),
            base_ids: Vec::new(),
            index: HashMap::new(),
            base_index: HashMap::new(),
            motion: resolve_tag(pack, "minecraft:blocks_motion_in_heightmap"),
            uncarvable: resolve_tag(pack, "minecraft:uncarvable"),
        };
        t.intern("minecraft:air");
        t
    }

    /// Identifier of a state (created as needed).
    pub fn intern(&mut self, state: &str) -> BlockId {
        // State already known: lookup without allocated string (`RUSTCRAFT_OPT_TAG_BORROW`).
        if let Some(i) = crate::data::with_full_id(state, |k| self.index.get(k).copied()) {
            return i;
        }
        let state = full_id(state);
        let b = block_of(&state);
        let mut f = 0;
        match b {
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => f |= FLAG_AIR,
            "minecraft:water" => f |= FLAG_FLUID | FLAG_WATER,
            "minecraft:lava" => f |= FLAG_FLUID | FLAG_LAVA,
            _ => {}
        }
        if state.contains("waterlogged=true") {
            f |= FLAG_FLUID;
        }
        if self.motion.contains(b) {
            f |= FLAG_MOTION;
        }
        if self.uncarvable.contains(b) {
            f |= FLAG_UNCARVABLE;
        }
        let id = BlockId::try_from(self.names.len()).expect("more than 65,536 states");
        let next_base = self.base_index.len() as u16;
        let base = *self.base_index.entry_ref(b).or_insert(next_base);
        self.names.push(state.clone());
        self.flags.push(f);
        self.base_ids.push(base);
        self.index.insert(state, id);
        id
    }

    pub fn get(&self, state: &str) -> Option<BlockId> {
        // Name already full: direct lookup, without the string allocated by `full_id`.
        if crate::opt::terrain_ids() && state.contains(':') {
            return self.index.get(state).copied();
        }
        self.index.get(&full_id(state)).copied()
    }

    pub fn name(&self, id: BlockId) -> &str {
        &self.names[id as usize]
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    #[inline]
    pub fn is_air(&self, id: BlockId) -> bool {
        self.flags[id as usize] & FLAG_AIR != 0
    }

    /// The state carries a fluid (water, lava, waterlogged block).
    #[inline]
    pub fn has_fluid(&self, id: BlockId) -> bool {
        self.flags[id as usize] & FLAG_FLUID != 0
    }

    #[inline]
    pub fn is_water(&self, id: BlockId) -> bool {
        self.flags[id as usize] & FLAG_WATER != 0
    }

    #[inline]
    pub fn is_lava(&self, id: BlockId) -> bool {
        self.flags[id as usize] & FLAG_LAVA != 0
    }

    /// Tag `blocks_motion_in_heightmap` (maps `OCEAN_FLOOR*`).
    #[inline]
    pub fn blocks_motion(&self, id: BlockId) -> bool {
        self.flags[id as usize] & FLAG_MOTION != 0
    }

    /// Tag `uncarvable` (carved caves do not touch it).
    #[inline]
    pub fn uncarvable(&self, id: BlockId) -> bool {
        self.flags[id as usize] & FLAG_UNCARVABLE != 0
    }

    /// Neither air nor fluid: a cell the surface rules are evaluated at.
    #[inline]
    pub fn is_solid(&self, id: BlockId) -> bool {
        self.flags[id as usize] & (FLAG_AIR | FLAG_FLUID) == 0
    }

    /// A solid the surface rules can overwrite in place: not air, no fluid, and it blocks motion
    /// (so it already counts in both generation height maps).
    #[inline]
    pub fn is_plain_solid(&self, id: BlockId) -> bool {
        self.flags[id as usize] & (FLAG_AIR | FLAG_FLUID | FLAG_MOTION) == FLAG_MOTION
    }

    /// Same block (properties ignored).
    #[inline]
    pub fn same_block(&self, a: BlockId, b: BlockId) -> bool {
        if crate::opt::base_block_ids() { self.same_block_by_base_id(a, b) } else { self.same_block_by_name(a, b) }
    }

    /// [`BlockTable::same_block`] by comparing the names cut at the properties (old path).
    pub fn same_block_by_name(&self, a: BlockId, b: BlockId) -> bool {
        a == b || block_of(self.name(a)) == block_of(self.name(b))
    }

    /// [`BlockTable::same_block`] by the block identifiers numbered at interning (`RUSTCRAFT_OPT_BASE_BLOCK_IDS`).
    #[inline]
    pub fn same_block_by_base_id(&self, a: BlockId, b: BlockId) -> bool {
        self.base_ids[a as usize] == self.base_ids[b as usize]
    }
}

/// Blocks of a block tag, nested tags expanded.
pub fn resolve_tag(pack: &DataPack, tag: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut seen = HashSet::new();
    let mut stack = vec![full_id(tag)];
    while let Some(t) = stack.pop() {
        if !seen.insert(t.clone()) {
            continue;
        }
        let Ok(j) = pack.json("tags/block", &t) else { continue };
        for v in j.get("values").and_then(Json::as_array).unwrap_or(&[]) {
            let s = v.as_str().or_else(|| v.get("id").and_then(Json::as_str)).unwrap_or_default();
            if let Some(inner) = s.strip_prefix('#') {
                stack.push(full_id(inner));
            } else if !s.is_empty() {
                out.insert(full_id(s));
            }
        }
    }
    out
}
