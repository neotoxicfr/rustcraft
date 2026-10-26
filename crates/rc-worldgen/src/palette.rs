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
    index: HashMap<String, BlockId>,
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
            index: HashMap::new(),
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
        self.names.push(state.clone());
        self.flags.push(f);
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

    /// Same block (properties ignored).
    pub fn same_block(&self, a: BlockId, b: BlockId) -> bool {
        a == b || block_of(self.name(a)) == block_of(self.name(b))
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
