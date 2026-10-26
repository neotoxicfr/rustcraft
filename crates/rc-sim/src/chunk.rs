//! Block chunk (`anvil-world.md` § 4): disk NBT → palettized sections with global
//! identifiers, light, height maps, block entities. The compound read is kept for
//! rewriting (unknown keys copied as they are, § 7).

use std::sync::Arc;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_world::{Container, LIGHT_BYTES, LightLayer, SECTION_BIOMES, SECTION_BLOCKS, Section, longs_for};

use crate::nbtio::{int, list};
use crate::states::States;

/// Data version of 26.3.
pub const DATA_VERSION: i32 = 5023;

/// Height maps: disk name ↔ network identifier.
pub const HEIGHTMAPS: [(&str, i32); 6] = [
    ("WORLD_SURFACE_WG", 0),
    ("WORLD_SURFACE", 1),
    ("OCEAN_FLOOR_WG", 2),
    ("OCEAN_FLOOR", 3),
    ("MOTION_BLOCKING", 4),
    ("MOTION_BLOCKING_NO_LEAVES", 5),
];

// Rule bits and live maps: shared with the client (`rc_world::heightmap`).
pub use rc_world::heightmap::{HM_MATERIAL_MOTION, HM_MOTION, HM_MOTION_NO_LEAVES, HM_NOT_AIR};
use rc_world::heightmap::LIVE_HEIGHTMAPS;

/// Per-state rules from the resolved block tags (sorted block identifiers).
pub fn heightmap_rules(data: &VersionData, block_tags: &hashbrown::HashMap<String, Vec<u32>>) -> Vec<u8> {
    let tag = |n: &str| block_tags.get(n).map(Vec::as_slice).unwrap_or(&[]);
    let (motion, motion_nl) =
        (tag("minecraft:blocks_motion_in_heightmap"), tag("minecraft:blocks_motion_in_heightmap_no_leaves"));
    let air: Vec<u16> = ["minecraft:air", "minecraft:cave_air", "minecraft:void_air"]
        .iter()
        .filter_map(|n| data.blocks.iter().position(|b| b.name == *n).map(|i| i as u16))
        .collect();
    data.states
        .iter()
        .map(|s| {
            let fluid = data.fluids.get(s.fluid as usize).is_some_and(|f| f != "minecraft:empty");
            let b = u32::from(s.block);
            let mut r = 0;
            if !air.contains(&s.block) {
                r |= HM_NOT_AIR;
            }
            if motion.binary_search(&b).is_ok() {
                r |= HM_MATERIAL_MOTION | HM_MOTION;
            }
            if fluid {
                r |= HM_MOTION | HM_MOTION_NO_LEAVES;
            }
            if motion_nl.binary_search(&b).is_ok() {
                r |= HM_MOTION_NO_LEAVES;
            }
            r
        })
        .collect()
}

/// Width of a height map value (ceiling of log2(height + 1)).
fn heightmap_bits(h: Height) -> usize {
    let n = h.sections as u32 * 16;
    (32 - n.leading_zeros()) as usize
}

fn heightmap_longs(h: Height) -> usize {
    let bits = heightmap_bits(h);
    256usize.div_ceil(64 / bits)
}

use rc_world::heightmap::{get as hm_get, set as hm_set};

/// Height of a dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Height {
    pub min_y: i32,
    pub sections: usize,
}

impl Height {
    pub fn min_section(&self) -> i32 {
        self.min_y >> 4
    }
}

#[derive(Debug, Clone)]
pub struct Chunk {
    pub x: i32,
    pub z: i32,
    pub sections: Vec<Section>,
    /// `sections + 2` positions (one below the world, one above).
    pub sky: Vec<LightLayer>,
    pub block: Vec<LightLayer>,
    pub light_on: bool,
    /// Network identifier and longs (disk format = network format).
    pub heightmaps: Vec<(i32, Box<[i64]>)>,
    pub block_entities: Vec<Nbt>,
    /// Inhabited time (`InhabitedTime`): +1 per tick as long as the chunk is within a player's
    /// spawn range; feeds the local difficulty.
    pub inhabited_time: i64,
    /// Compound read, kept for rewriting.
    pub root: Nbt,
    /// An unknown identifier was replaced: this chunk is never rewritten.
    pub lossy: bool,
    /// Sections whose blocks changed since the read (re-encoded on write).
    pub changed: Vec<bool>,
    /// Modified since the last save.
    pub dirty: bool,
}

/// Why a present chunk is not loaded (treated as absent, never rewritten).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Status other than `full` (unfinished generation).
    NotFull(String),
    OldVersion(i64),
    Corrupt(String),
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Skip::NotFull(s) => write!(f, "status '{s}'"),
            Skip::OldVersion(v) => write!(f, "data version {v}"),
            Skip::Corrupt(e) => write!(f, "corrupt: {e}"),
        }
    }
}

fn bits_for(n: usize) -> u8 {
    (usize::BITS - (n.max(1) - 1).leading_zeros()) as u8
}

/// Disk container (§ 4.3) → memory container with global identifiers. `lossy`: an unknown
/// entry was replaced by `fallback`.
fn container(
    c: Option<&Nbt>,
    entries: usize,
    blocks: bool,
    fallback: u32,
    mut resolve: impl FnMut(&Nbt) -> Option<u32>,
    lossy: &mut bool,
) -> Result<Container, String> {
    let Some(c) = c else { return Ok(Container::Single(fallback)) };
    let palette = list(c.get("palette"));
    if palette.is_empty() {
        return Err("empty palette".into());
    }
    let ids: Box<[u32]> = palette
        .iter()
        .map(|e| {
            resolve(e).unwrap_or_else(|| {
                *lossy = true;
                fallback
            })
        })
        .collect();
    if ids.len() == 1 {
        return Ok(Container::Single(ids[0]));
    }
    let k = bits_for(ids.len());
    let bits = if blocks && k <= 4 { 4 } else { k };
    let data: Box<[u64]> = match c.get("data") {
        Some(Nbt::LongArray(d)) => d.iter().map(|&v| v as u64).collect(),
        _ => return Err("data missing".into()),
    };
    if data.len() != longs_for(entries, bits) {
        return Err(format!("{} longs for {bits} bits", data.len()));
    }
    // Indices outside the palette: corrupt.
    let (vpl, mask) = (64 / bits as usize, (1u64 << bits) - 1);
    for (w, word) in data.iter().enumerate() {
        for j in 0..vpl.min(entries - (w * vpl).min(entries)) {
            if ((word >> (j * bits as usize)) & mask) as usize >= ids.len() {
                return Err("index outside the palette".into());
            }
        }
    }
    Container::packed(entries, bits, Some(ids), data).ok_or_else(|| "container".into())
}

fn light_layer(v: Option<&Nbt>) -> Result<LightLayer, String> {
    match v {
        None => Ok(None),
        Some(Nbt::ByteArray(b)) if b.len() == LIGHT_BYTES => {
            let mut a = [0u8; LIGHT_BYTES];
            a.copy_from_slice(b);
            Ok(Some(Arc::new(a)))
        }
        Some(_) => Err("light layer of invalid size".into()),
    }
}

/// Number of non-empty blocks of a section.
pub fn count_non_empty(blocks: &Container, states: &States) -> i16 {
    match blocks {
        Container::Single(v) => {
            if states.is_air(*v) {
                0
            } else {
                SECTION_BLOCKS as i16
            }
        }
        c => {
            let mut v = [0u32; SECTION_BLOCKS];
            c.unpack_into(&mut v);
            v.iter().filter(|&&s| !states.is_air(s)).count() as i16
        }
    }
}

/// Memory container → disk `{ palette, data? }` (§ 4.3): palette recompacted in order
/// of first appearance, width according to the disk rules.
fn encode_container(c: &Container, entries: usize, blocks: bool, entry: impl Fn(u32) -> Nbt) -> Nbt {
    if disk_remap() {
        encode_container_remap(c, entries, blocks, entry)
    } else {
        encode_container_reference(c, entries, blocks, entry)
    }
}

/// `RUSTCRAFT_OPT_DISKREMAP=0`: old disk encoder (unfolding, fresh hash table per
/// container). Default: canonical palette derived from the raw indices, same bytes.
fn disk_remap() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_DISKREMAP") != Some("0"))
}

/// Same bytes as [`encode_container_reference`], without unfolding or hash table as long as the
/// canonical palette fits in 256 entries (beyond: old path).
fn encode_container_remap(c: &Container, entries: usize, blocks: bool, entry: impl Fn(u32) -> Nbt) -> Nbt {
    let mut palette = [0u32; 256];
    let mut idx = [0u8; SECTION_BLOCKS];
    let Some(n) = c.first_seen_indices(entries, 256, 16, &mut palette, &mut idx) else {
        return encode_container_reference(c, entries, blocks, entry);
    };
    let entries_nbt: Vec<Nbt> = palette[..n].iter().map(|&v| entry(v)).collect();
    let mut out = vec![("palette".to_owned(), Nbt::List(entries_nbt))];
    if n > 1 {
        let k = bits_for(n);
        let b = if blocks && k <= 4 { 4 } else { k };
        let vpl = 64 / b as usize;
        let data: Vec<i64> = idx[..entries]
            .chunks(vpl)
            .map(|chunk| {
                let mut acc = 0u64;
                for (j, &r) in chunk.iter().enumerate() {
                    acc |= u64::from(r) << (j * b as usize);
                }
                acc as i64
            })
            .collect();
        out.push(("data".to_owned(), Nbt::LongArray(data)));
    }
    Nbt::Compound(out)
}

/// Old disk encoder (reference of the proofs and `RUSTCRAFT_OPT_DISKREMAP=0`).
fn encode_container_reference(c: &Container, entries: usize, blocks: bool, entry: impl Fn(u32) -> Nbt) -> Nbt {
    let mut values = vec![0u32; entries];
    c.unpack_into(&mut values);
    let mut palette: Vec<u32> = Vec::new();
    let mut index: hashbrown::HashMap<u32, u64> = hashbrown::HashMap::new();
    let raw: Vec<u64> = values
        .iter()
        .map(|v| {
            *index.entry(*v).or_insert_with(|| {
                palette.push(*v);
                (palette.len() - 1) as u64
            })
        })
        .collect();
    // Mixed palette (strings and compounds): binary writing wraps the strings `{ "": … }` (§ 1).
    let entries_nbt: Vec<Nbt> = palette.iter().map(|&v| entry(v)).collect();
    let mut out = vec![("palette".to_owned(), Nbt::List(entries_nbt))];
    if palette.len() > 1 {
        let k = bits_for(palette.len());
        let b = if blocks && k <= 4 { 4 } else { k };
        let vpl = 64 / b as usize;
        let mut data = vec![0i64; longs_for(entries, b)];
        for (i, r) in raw.iter().enumerate() {
            data[i / vpl] |= (r << ((i % vpl) * b as usize)) as i64;
        }
        out.push(("data".to_owned(), Nbt::LongArray(data)));
    }
    Nbt::Compound(out)
}

/// Block container in disk format.
pub fn encode_blocks(c: &Container, data: &VersionData, states: &States) -> Nbt {
    encode_container(c, SECTION_BLOCKS, true, |v| states.palette_entry(data, v))
}

/// Placement without notification in a chunk of the list (flag 2 of the game): height maps
/// kept up to date, block entity of the old state removed. False outside the given chunks.
fn set_in(
    chunks: &mut [Chunk],
    p: [i32; 3],
    state: u32,
    data: &VersionData,
    states: &States,
    rules: &[u8],
    h: Height,
) -> bool {
    let ly = p[1] - h.min_y;
    let Some(c) = chunks.iter_mut().find(|c| (c.x, c.z) == (p[0] >> 4, p[2] >> 4)) else { return false };
    let Some(si) = usize::try_from(ly >> 4).ok().filter(|&i| i < c.sections.len()) else { return false };
    let idx = rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize);
    if c.sections[si].blocks.get(idx) == state {
        return true;
    }
    c.sections[si].set_blocks(&[(idx as u16, state)], data.state_bits() as u8, |s| states.is_air(s));
    c.update_heightmaps((p[0] & 15) as usize, p[1], (p[2] & 15) as usize, state, rules, h);
    c.changed[si] = true;
    c.dirty = true;
    c.block_entities.retain(|b| [int(b, "x"), int(b, "y"), int(b, "z")] != p.map(|v| Some(i64::from(v))));
    true
}

fn get_in(chunks: &[Chunk], p: [i32; 3], h: Height) -> Option<u32> {
    let ly = p[1] - h.min_y;
    let c = chunks.iter().find(|c| (c.x, c.z) == (p[0] >> 4, p[2] >> 4))?;
    let s = c.sections.get(usize::try_from(ly >> 4).ok()?)?;
    Some(s.blocks.get(rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize)))
}

/// Bonus chest of world creation (`bonus_chest` feature, placed once by the server at the
/// spawn point): columns x then z of the chunk shuffled, first free top of the "motion
/// blocking, no leaves" map (air or no collision); loot chest
/// `chests/spawn_bonus_chest` (seed drawn), torches to the north, east, south and west where
/// they hold (central support below). Returns the touched chunks (empty: nothing placed).
/// `Err(chunk)`: a torch would fall into a neighbouring chunk absent from `chunks` (the game
/// generates it up to the full status to place it); the caller adds it and starts again with the same
/// seed, with nothing having been placed. `unavailable`: neighbours that could not be produced
/// (no torch on that side).
pub fn place_bonus_chest(
    chunks: &mut [Chunk],
    spawn: [i32; 3],
    data: &VersionData,
    states: &States,
    (rules, unavailable): (&[u8], &[(i32, i32)]),
    h: Height,
    rng: &mut crate::rng::Rng,
) -> Result<Vec<(i32, i32)>, (i32, i32)> {
    let (cx, cz) = (spawn[0] >> 4, spawn[2] >> 4);
    let Some(ci) = chunks.iter().position(|c| (c.x, c.z) == (cx, cz)) else { return Ok(Vec::new()) };
    let mut shuffled = |base: i32| {
        let mut v: Vec<i32> = (base..base + 16).collect();
        for i in (2..=v.len()).rev() {
            let j = rng.next_int_bound(i as i32) as usize;
            v.swap(i - 1, j);
        }
        v
    };
    let (xs, zs) = (shuffled(cx * 16), shuffled(cz * 16));
    let id = |n: &str| data.blocks.iter().find(|b| b.name == n).map(|b| b.default_state);
    let (Some(chest), Some(torch)) = (id("minecraft:chest"), id("minecraft:torch")) else { return Ok(Vec::new()) };
    let bits = heightmap_bits(h);
    let top_y = h.min_y + h.sections as i32 * 16;
    for &x in &xs {
        for &z in &zs {
            let c = &chunks[ci];
            let Some((_, longs)) = c.heightmaps.iter().find(|(k, _)| *k == 5) else { return Ok(Vec::new()) };
            let y = hm_get(longs, ((z & 15) * 16 + (x & 15)) as usize, bits) + h.min_y;
            let pos = [x, y, z];
            let cur = if y < top_y { get_in(chunks, pos, h).unwrap_or(states.air_state) } else { states.air_state };
            let free =
                states.is_air(cur) || data.states.get(cur as usize).is_none_or(|s| data.shape(s.collision).is_empty());
            if !free {
                continue;
            }
            for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                let q = ((x + dx) >> 4, (z + dz) >> 4);
                if !chunks.iter().any(|c| (c.x, c.z) == q) && !unavailable.contains(&q) {
                    return Err(q);
                }
            }
            let mut touched = vec![(cx, cz)];
            if !set_in(chunks, pos, chest, data, states, rules, h) {
                // Above the world: the game places nothing there either, and stops there.
                return Ok(Vec::new());
            }
            let seed = rng.next_long();
            chunks[ci].block_entities.push(Nbt::Compound(vec![
                ("id".into(), Nbt::String("minecraft:chest".into())),
                ("x".into(), Nbt::Int(x)),
                ("y".into(), Nbt::Int(y)),
                ("z".into(), Nbt::Int(z)),
                ("keepPacked".into(), Nbt::Byte(0)),
                ("LootTable".into(), Nbt::String("minecraft:chests/spawn_bonus_chest".into())),
                ("LootTableSeed".into(), Nbt::Long(seed)),
            ]));
            for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
                let tp = [x + dx, y, z + dz];
                let support = get_in(chunks, [tp[0], y - 1, tp[2]], h)
                    .is_some_and(|b| data.sim.get(b as usize).is_some_and(|v| v & rc_data::sim_bits::CENTER_UP != 0));
                if support
                    && set_in(chunks, tp, torch, data, states, rules, h)
                    && !touched.contains(&(tp[0] >> 4, tp[2] >> 4))
                {
                    touched.push((tp[0] >> 4, tp[2] >> 4));
                }
            }
            return Ok(touched);
        }
    }
    Ok(Vec::new())
}

impl Chunk {
    fn state_at(&self, lx: usize, ly: usize, lz: usize) -> u32 {
        self.sections[ly >> 4].blocks.get(rc_world::block_index(lx, ly & 15, lz))
    }

    /// All blocks, sections from bottom to top (same order as the light columns).
    pub fn unpack_blocks(&self) -> Vec<u32> {
        let mut out = vec![0u32; self.sections.len() * SECTION_BLOCKS];
        for (i, s) in self.sections.iter().enumerate() {
            s.blocks.unpack_into(&mut out[i * SECTION_BLOCKS..(i + 1) * SECTION_BLOCKS]);
        }
        out
    }

    /// Primes the missing or wrongly sized live maps: sweep of each column
    /// from the top of the highest non-empty section, skipping pure air.
    pub fn prime_heightmaps(&mut self, rules: &[u8], air: u32, h: Height) {
        if rules.is_empty() {
            return;
        }
        let want = heightmap_longs(h);
        let missing: Vec<(i32, u8)> = LIVE_HEIGHTMAPS
            .iter()
            .copied()
            .filter(|(id, _)| !self.heightmaps.iter().any(|(k, l)| k == id && l.len() == want))
            .collect();
        if missing.is_empty() {
            return;
        }
        self.heightmaps.retain(|(k, _)| !missing.iter().any(|(id, _)| id == k));
        let bits = heightmap_bits(h);
        let top = self.sections.iter().rposition(|s| !s.is_empty()).map_or(0, |i| (i + 1) * 16);
        let mut maps: Vec<(i32, u8, Vec<i64>)> = missing.iter().map(|&(id, r)| (id, r, vec![0i64; want])).collect();
        for z in 0..16 {
            for x in 0..16 {
                let mut left: u8 = missing.iter().fold(0, |a, &(_, r)| a | r);
                let mut done = 0usize;
                for ly in (0..top).rev() {
                    let s = self.state_at(x, ly, z);
                    if s == air {
                        continue;
                    }
                    let r = rules.get(s as usize).copied().unwrap_or(0);
                    for m in maps.iter_mut() {
                        if left & m.1 != 0 && r & m.1 != 0 {
                            hm_set(&mut m.2, z * 16 + x, bits, ly as i32 + 1);
                            left &= !m.1;
                            done += 1;
                        }
                    }
                    if done == maps.len() {
                        break;
                    }
                }
            }
        }
        for (id, _, l) in maps {
            self.heightmaps.push((id, l.into_boxed_slice()));
        }
    }

    /// Update of the live maps after the placement of `state` at (lx, y, lz) (world y
    /// coordinate), in the order of the game: motion blocking, no leaves, ocean floor, surface.
    pub fn update_heightmaps(&mut self, lx: usize, y: i32, lz: usize, state: u32, rules: &[u8], h: Height) {
        if rules.is_empty() {
            return;
        }
        let bits = heightmap_bits(h);
        let i = lz * 16 + lx;
        let r = rules.get(state as usize).copied().unwrap_or(0);
        for (id, rule) in LIVE_HEIGHTMAPS {
            let Some(k) = self.heightmaps.iter().position(|(m, l)| *m == id && l.len() == heightmap_longs(h)) else {
                continue;
            };
            let mut longs = std::mem::take(&mut self.heightmaps[k].1);
            rc_world::heightmap::update(&mut longs, bits, i, y, h.min_y, r & rule != 0, |yy| {
                let s = self.state_at(lx, (yy - h.min_y) as usize, lz);
                rules.get(s as usize).copied().unwrap_or(0) & rule != 0
            });
            self.heightmaps[k].1 = longs;
        }
    }

    /// Compound to write (§ 7): the compound read, with the changed sections re-encoded, the current light,
    /// the current block entities, `LastUpdate`, the current height maps (without up-to-date
    /// live maps, those of a modified chunk are removed to be recomputed).
    /// `None`: chunk never to be rewritten (unknown identifier replaced).
    pub fn encode(&self, data: &VersionData, states: &States, h: Height, time: i64) -> Option<Nbt> {
        if self.lossy {
            return None;
        }
        let mut root = self.root.clone();
        let min = h.min_section();
        let old: Vec<Nbt> = list(root.get("sections")).to_vec();
        let mut sections = Vec::new();
        for li in 0..self.sky.len() as i32 {
            let y = min - 1 + li;
            let found = old.iter().find(|s| int(s, "Y") == Some(i64::from(y))).cloned();
            let existed = found.is_some();
            let mut s = found.unwrap_or_else(|| Nbt::Compound(vec![("Y".into(), Nbt::Byte(y as i8))]));
            let i = y - min;
            if (0..h.sections as i32).contains(&i) {
                let sec = &self.sections[i as usize];
                if self.changed[i as usize] || (!existed && !sec.is_empty()) {
                    let blocks = encode_container(&sec.blocks, SECTION_BLOCKS, true, |v| states.palette_entry(data, v));
                    crate::nbtio::set(&mut s, "block_states", blocks);
                    if s.get("biomes").is_none() {
                        let names = &states.biome_names;
                        let biomes = encode_container(&sec.biomes, SECTION_BIOMES, false, |v| {
                            Nbt::String(names.get(v as usize).cloned().unwrap_or_else(|| "minecraft:plains".into()))
                        });
                        crate::nbtio::set(&mut s, "biomes", biomes);
                    }
                }
            }
            for (key, layer) in [("BlockLight", &self.block[li as usize]), ("SkyLight", &self.sky[li as usize])] {
                if let Some(l) = layer {
                    crate::nbtio::set(&mut s, key, Nbt::ByteArray(l.to_vec()));
                }
            }
            if matches!(&s, Nbt::Compound(f) if f.len() > 1) {
                sections.push(s);
            }
        }
        crate::nbtio::set(&mut root, "sections", Nbt::List(sections));
        crate::nbtio::set(&mut root, "block_entities", Nbt::List(self.block_entities.clone()));
        crate::nbtio::set(&mut root, "LastUpdate", Nbt::Long(time));
        crate::nbtio::set(&mut root, "DataVersion", Nbt::Int(DATA_VERSION));
        let live = LIVE_HEIGHTMAPS
            .iter()
            .all(|(id, _)| self.heightmaps.iter().any(|(k, l)| k == id && l.len() == heightmap_longs(h)));
        if live {
            let maps = HEIGHTMAPS
                .iter()
                .filter_map(|(name, id)| {
                    let (_, l) = self.heightmaps.iter().find(|(k, _)| k == id)?;
                    Some(((*name).to_owned(), Nbt::LongArray(l.to_vec())))
                })
                .collect();
            crate::nbtio::set(&mut root, "Heightmaps", Nbt::Compound(maps));
        } else if self.changed.iter().any(|&c| c) {
            crate::nbtio::remove(&mut root, "Heightmaps");
        }
        crate::nbtio::set(&mut root, "InhabitedTime", Nbt::Long(self.inhabited_time));
        Some(root)
    }
}

/// Reads a chunk. Error: to be treated as absent (and never rewritten).
pub fn parse(root: Nbt, data: &VersionData, states: &States, h: Height) -> Result<Chunk, Skip> {
    let status = root.get("Status").and_then(Nbt::as_str).unwrap_or("");
    if status.strip_prefix("minecraft:").unwrap_or(status) != "full" {
        return Err(Skip::NotFull(status.to_owned()));
    }
    let version = int(&root, "DataVersion").unwrap_or(0);
    if version < i64::from(DATA_VERSION) {
        return Err(Skip::OldVersion(version));
    }
    let corrupt = Skip::Corrupt;
    let x = int(&root, "xPos").ok_or_else(|| corrupt("xPos".into()))? as i32;
    let z = int(&root, "zPos").ok_or_else(|| corrupt("zPos".into()))? as i32;
    let n = h.sections;
    let min = h.min_section();
    let mut lossy = false;
    let mut sections: Vec<Section> = (0..n).map(|_| Section::empty(states.air_state, states.default_biome)).collect();
    let mut sky: Vec<LightLayer> = vec![None; n + 2];
    let mut block: Vec<LightLayer> = vec![None; n + 2];
    for s in list(root.get("sections")) {
        let y = int(s, "Y").ok_or_else(|| corrupt("section without Y".into()))? as i32;
        let li = y - (min - 1);
        if (0..n as i32 + 2).contains(&li) {
            sky[li as usize] = light_layer(s.get("SkyLight")).map_err(corrupt)?;
            block[li as usize] = light_layer(s.get("BlockLight")).map_err(corrupt)?;
        }
        let i = y - min;
        if !(0..n as i32).contains(&i) {
            continue;
        }
        let blocks = container(
            s.get("block_states"),
            SECTION_BLOCKS,
            true,
            states.air_state,
            |e| states.state_of(data, e),
            &mut lossy,
        )
        .map_err(|e| corrupt(format!("section {y}, blocks: {e}")))?;
        let biomes = container(
            s.get("biomes"),
            SECTION_BIOMES,
            false,
            states.default_biome,
            |e| e.as_str().and_then(|n| states.biomes.get(n).copied()),
            &mut lossy,
        )
        .map_err(|e| corrupt(format!("section {y}, biomes: {e}")))?;
        let non_empty = count_non_empty(&blocks, states);
        sections[i as usize] = Section { non_empty, blocks, biomes };
    }
    let heightmaps = match root.get("Heightmaps") {
        Some(Nbt::Compound(f)) => f
            .iter()
            .filter_map(|(k, v)| {
                let id = HEIGHTMAPS.iter().find(|(name, _)| name == k)?.1;
                match v {
                    Nbt::LongArray(a) => Some((id, a.clone().into_boxed_slice())),
                    _ => None,
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let block_entities = list(root.get("block_entities")).to_vec();
    let light_on = int(&root, "isLightOn") == Some(1);
    let inhabited_time = int(&root, "InhabitedTime").unwrap_or(0);
    let mut c = Chunk {
        x,
        z,
        sections,
        sky,
        block,
        light_on,
        heightmaps,
        block_entities,
        inhabited_time,
        root,
        lossy,
        changed: vec![false; n],
        dirty: false,
    };
    c.prime_heightmaps(&states.heightmap_rules, states.air_state, h);
    Ok(c)
}

#[cfg(test)]
mod heightmap_tests {
    use super::*;
    use crate::states::States;

    fn setup() -> Option<(VersionData, States)> {
        let data = rc_testdata::version_data()?;
        let regs = crate::registries::load(&crate::registries::client_jar(&data)?, &data).ok()?;
        let tags: hashbrown::HashMap<String, Vec<u32>> = regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:block")
            .map(|(_, t)| {
                t.iter()
                    .map(|(n, ids)| {
                        let mut v: Vec<u32> = ids.iter().map(|&i| i as u32).collect();
                        v.sort_unstable();
                        (n.clone(), v)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let states = States::new(&data, vec!["minecraft:plains".into()]).with_heightmap_rules(&data, &tags);
        Some((data, states))
    }

    const H: Height = Height { min_y: -64, sections: 24 };

    fn state(data: &VersionData, name: &str) -> u32 {
        data.blocks.iter().find(|b| b.name == name).unwrap().default_state
    }

    fn set(c: &mut Chunk, data: &VersionData, states: &States, x: usize, y: i32, z: usize, s: u32) {
        let ly = (y - H.min_y) as usize;
        let idx = rc_world::block_index(x, ly & 15, z) as u16;
        if c.sections[ly >> 4].blocks.get(idx as usize) == s {
            return;
        }
        c.sections[ly >> 4].set_blocks(&[(idx, s)], data.state_bits() as u8, |v| states.is_air(v));
        c.update_heightmaps(x, y, z, s, &states.heightmap_rules, H);
        c.changed[ly >> 4] = true;
    }

    fn flat(data: &VersionData, states: &States) -> Chunk {
        let mut c = Chunk {
            x: 0,
            z: 0,
            sections: (0..H.sections).map(|_| Section::empty(states.air_state, 0)).collect(),
            sky: vec![None; H.sections + 2],
            block: vec![None; H.sections + 2],
            light_on: true,
            heightmaps: Vec::new(),
            block_entities: Vec::new(),
            inhabited_time: 0,
            root: Nbt::Compound(vec![("Status".into(), Nbt::String("minecraft:full".into()))]),
            lossy: false,
            changed: vec![false; H.sections],
            dirty: false,
        };
        let layers =
            ["minecraft:bedrock", "minecraft:stone", "minecraft:stone", "minecraft:stone", "minecraft:grass_block"];
        for (i, n) in layers.iter().enumerate() {
            let s = state(data, n);
            for z in 0..16 {
                for x in 0..16 {
                    let ly = i;
                    let idx = rc_world::block_index(x, ly, z) as u16;
                    c.sections[0].set_blocks(&[(idx, s)], data.state_bits() as u8, |v| states.is_air(v));
                }
            }
        }
        c.prime_heightmaps(&states.heightmap_rules, states.air_state, H);
        c
    }

    fn height(c: &Chunk, id: i32, x: usize, z: usize) -> i32 {
        let (_, l) = c.heightmaps.iter().find(|(k, _)| *k == id).unwrap();
        hm_get(l, z * 16 + x, heightmap_bits(H)) + H.min_y
    }

    fn reprimed(c: &Chunk, states: &States) -> Vec<(i32, Box<[i64]>)> {
        let mut f = c.clone();
        f.heightmaps.clear();
        f.prime_heightmaps(&states.heightmap_rules, states.air_state, H);
        f.heightmaps.sort_by_key(|(k, _)| *k);
        f.heightmaps
    }

    /// The rules follow the tags and fluids of the game.
    #[test]
    fn rules_follow_tags_and_fluids() {
        let Some((data, states)) = setup() else { return };
        let r = |n: &str| states.heightmap_rules[state(&data, n) as usize];
        assert_eq!(r("minecraft:air"), 0);
        assert_eq!(r("minecraft:cave_air"), 0);
        assert_eq!(r("minecraft:stone"), HM_NOT_AIR | HM_MATERIAL_MOTION | HM_MOTION | HM_MOTION_NO_LEAVES);
        assert_eq!(r("minecraft:oak_leaves"), HM_NOT_AIR | HM_MATERIAL_MOTION | HM_MOTION);
        assert_eq!(r("minecraft:water"), HM_NOT_AIR | HM_MOTION | HM_MOTION_NO_LEAVES);
        assert_eq!(r("minecraft:torch"), HM_NOT_AIR);
    }

    /// Placement of a roof then removal: the maps follow like the game (the roof shelters, its removal
    /// goes back down to the highest "opaque" cell for each map).
    #[test]
    fn roof_updates_live_heightmaps() {
        let Some((data, states)) = setup() else { return };
        let mut c = flat(&data, &states);
        assert_eq!(height(&c, 4, 3, 3), -59);
        set(&mut c, &data, &states, 3, 10, 3, state(&data, "minecraft:oak_leaves"));
        assert_eq!(height(&c, 4, 3, 3), 11, "motion blocking");
        assert_eq!(height(&c, 5, 3, 3), -59, "no leaves");
        assert_eq!(height(&c, 1, 3, 3), 11, "surface");
        set(&mut c, &data, &states, 3, 20, 3, state(&data, "minecraft:torch"));
        assert_eq!(height(&c, 1, 3, 3), 21);
        assert_eq!(height(&c, 4, 3, 3), 11);
        set(&mut c, &data, &states, 3, 20, 3, states.air_state);
        assert_eq!(height(&c, 1, 3, 3), 11);
        set(&mut c, &data, &states, 3, 10, 3, states.air_state);
        assert_eq!(height(&c, 4, 3, 3), -59);
        assert_eq!(height(&c, 1, 3, 3), -59);
    }

    /// Sequence of random placements: the live maps equal a fresh priming.
    #[test]
    fn random_edits_match_fresh_priming() {
        let Some((data, states)) = setup() else { return };
        let mut c = flat(&data, &states);
        let palette = [
            "minecraft:stone",
            "minecraft:air",
            "minecraft:water",
            "minecraft:oak_leaves",
            "minecraft:torch",
            "minecraft:glass",
            "minecraft:cave_air",
            "minecraft:short_grass",
        ]
        .map(|n| state(&data, n));
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for step in 0..20_000 {
            // Columns grouped to multiply removals at the top.
            let (x, z) = (next(3) as usize, next(3) as usize);
            let y = -64 + next(40) as i32;
            let s = palette[next(palette.len() as u64) as usize];
            set(&mut c, &data, &states, x, y, z, s);
            if step % 500 == 0 {
                let mut live = c.heightmaps.clone();
                live.sort_by_key(|(k, _)| *k);
                assert_eq!(live, reprimed(&c, &states), "step {step}");
            }
        }
    }

    /// On write, a modified chunk keeps its current maps (no further recompute needed).
    #[test]
    fn encode_keeps_current_heightmaps() {
        let Some((data, states)) = setup() else { return };
        let mut c = flat(&data, &states);
        set(&mut c, &data, &states, 0, 5, 0, state(&data, "minecraft:stone"));
        let root = c.encode(&data, &states, H, 0).unwrap();
        let maps = root.get("Heightmaps").expect("maps written");
        for (name, id) in
            [("MOTION_BLOCKING", 4), ("WORLD_SURFACE", 1), ("OCEAN_FLOOR", 3), ("MOTION_BLOCKING_NO_LEAVES", 5)]
        {
            let Some(Nbt::LongArray(l)) = maps.get(name) else { panic!("{name}") };
            assert_eq!(hm_get(l, 0, heightmap_bits(H)) + H.min_y, 6, "{name}");
            let _ = id;
        }
    }

    /// Official world (`RUSTCRAFT_SOLO_WORLD`): priming gives back the maps recorded by
    /// the game, to the long, for all full chunks.
    #[test]
    fn priming_matches_official_world() {
        let Some(world) = crate::tests::test_world() else { return };
        let Some((data, states)) = setup() else { return };
        let mut n = 0;
        for name in crate::tests::DIMENSIONS {
            let dim = crate::world::Dimension::new(&world, name, crate::dimension_height(&data, name));
            let h = crate::dimension_height(&data, name);
            for (_, l) in dim.load_everything(&data, &states) {
                let crate::world::Load::Loaded(c) = l else { continue };
                let mut stored = c.heightmaps.clone();
                stored.retain(|(k, _)| LIVE_HEIGHTMAPS.iter().any(|(id, _)| id == k));
                stored.sort_by_key(|(k, _)| *k);
                let mut f = (*c).clone();
                f.heightmaps.clear();
                f.prime_heightmaps(&states.heightmap_rules, states.air_state, h);
                f.heightmaps.sort_by_key(|(k, _)| *k);
                assert_eq!(stored, f.heightmaps, "{name} ({}, {})", c.x, c.z);
                n += 1;
            }
        }
        eprintln!("{n} chunks: primed maps = maps of the game");
        assert!(n > 0);
    }
}

#[cfg(test)]
mod bonus_tests {
    use std::sync::Arc;

    use rc_proto::nbt::Nbt;

    /// Demo worlds for the client captures (`RUSTCRAFT_W12_PREP=<saves folder>`):
    /// `w12-bonus` (gaze set on the bonus chest), `w12-debug`, `w12-void`.
    #[test]
    #[ignore = "capture preparation"]
    fn prep_capture_worlds() {
        let Some(saves) = rc_data::env_str!("RUSTCRAFT_W12_PREP") else { return };
        let saves = std::path::PathBuf::from(saves);
        let data = Arc::new(rc_testdata::version_data().expect("data"));
        let set_spawn = |dir: &std::path::Path, pos: [i32; 3], yaw: f32, pitch: f32| {
            let path = dir.join("level.dat");
            let mut root = crate::nbtio::read_dat(&path).expect("level.dat");
            let d = crate::nbtio::get_mut(&mut root, "Data").unwrap();
            let s = crate::nbtio::get_mut(d, "spawn").unwrap();
            crate::nbtio::set(s, "pos", Nbt::IntArray(pos.to_vec()));
            crate::nbtio::set(s, "yaw", Nbt::Float(yaw));
            crate::nbtio::set(s, "pitch", Nbt::Float(pitch));
            std::fs::write(&path, crate::nbtio::gzip(&crate::nbtio::encode_root(&root))).unwrap();
        };
        let radius0 = vec![("minecraft:respawn_radius".to_owned(), Nbt::Int(0))];
        for (name, preset, mode) in [
            ("w12-bonus", crate::newworld::Preset::Normal, 1),
            ("w12-debug", crate::newworld::Preset::Debug, 3),
            ("w12-void", crate::newworld::Preset::Flat, 1),
        ] {
            let dir = saves.join(name);
            let _ = std::fs::remove_dir_all(&dir);
            let mut w = crate::newworld::NewWorld::new(name);
            w.seed = 7;
            w.preset = preset;
            w.game_mode = mode;
            w.game_rules = radius0.clone();
            w.bonus_chest = name == "w12-bonus";
            if name == "w12-void" {
                w.flat_settings = Some(Nbt::Compound(vec![
                    ("biome".into(), Nbt::String("minecraft:the_void".into())),
                    ("features".into(), Nbt::Byte(1)),
                    ("lakes".into(), Nbt::Byte(0)),
                    (
                        "layers".into(),
                        Nbt::List(vec![Nbt::Compound(vec![
                            ("block".into(), Nbt::String("minecraft:air".into())),
                            ("height".into(), Nbt::Int(1)),
                        ])]),
                    ),
                    ("structure_overrides".into(), Nbt::List(Vec::new())),
                ]));
            }
            crate::newworld::create_world(&dir, &data, &w).expect("creation");
            let level = crate::level::Level::read(&dir).unwrap();
            match name {
                "w12-bonus" => {
                    let h = crate::dimension_height(&data, "minecraft:overworld");
                    let dim = crate::world::Dimension::new(&dir, "minecraft:overworld", h);
                    let states = crate::states::States::new(&data, Vec::new());
                    let s = level.spawn_pos;
                    let loaded = dim.load(&[(s[0] >> 4, s[2] >> 4)], &data, &states);
                    let crate::world::Load::Loaded(c) = &loaded[0].1 else { panic!() };
                    let b = c.block_entities.iter().find(|b| b.get("LootTable").is_some()).unwrap();
                    let p = ["x", "y", "z"].map(|k| super::int(b, k).unwrap() as i32);
                    // Three blocks to the south, gaze towards the north and a bit downwards.
                    set_spawn(&dir, [p[0], p[1] + 1, p[2] + 3], 180.0, 30.0);
                }
                "w12-debug" => set_spawn(&dir, [0, 80, 0], -45.0, 35.0),
                _ => set_spawn(&dir, [8, -60, 20], 180.0, 25.0),
            }
            eprintln!("{name} ready: spawn {:?}", level.spawn_pos);
        }
    }

    /// World created with the bonus chest: a loot chest `spawn_bonus_chest` at the top of a
    /// column of the spawn chunk, torches placed around (central support), lit, and
    /// height maps that count the chest.
    #[test]
    fn bonus_chest_placed_at_spawn() {
        let Some(data) = rc_testdata::version_data() else { return };
        let data = Arc::new(data);
        if crate::registries::client_jar(&data).is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rc-bonus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut w = crate::newworld::NewWorld::new("bonus");
        w.seed = 7;
        w.bonus_chest = true;
        crate::newworld::create_world(&dir, &data, &w).expect("creation");
        let level = crate::level::Level::read(&dir).expect("level.dat");
        let s = level.spawn_pos;
        let h = crate::dimension_height(&data, "minecraft:overworld");
        let dim = crate::world::Dimension::new(&dir, "minecraft:overworld", h);
        let states = crate::states::States::new(&data, Vec::new());
        let col = (s[0] >> 4, s[2] >> 4);
        let loaded = dim.load(&[col], &data, &states);
        let crate::world::Load::Loaded(c) = &loaded[0].1 else { panic!("{:?}", loaded[0].1) };
        let chests: Vec<_> = c
            .block_entities
            .iter()
            .filter(|b| b.get("LootTable").and_then(super::Nbt::as_str) == Some("minecraft:chests/spawn_bonus_chest"))
            .collect();
        assert_eq!(chests.len(), 1, "a single bonus chest");
        let p = [
            super::int(chests[0], "x").unwrap() as i32,
            super::int(chests[0], "y").unwrap() as i32,
            super::int(chests[0], "z").unwrap() as i32,
        ];
        let name = |q: [i32; 3]| -> String {
            let ly = q[1] - h.min_y;
            let st = c.sections[(ly >> 4) as usize].blocks.get(rc_world::block_index(
                (q[0] & 15) as usize,
                (ly & 15) as usize,
                (q[2] & 15) as usize,
            ));
            data.block_of(st).unwrap().name.clone()
        };
        assert_eq!(name(p), "minecraft:chest");
        let (_, mb) = c.heightmaps.iter().find(|(k, _)| *k == 4).expect("map");
        assert_eq!(
            super::hm_get(mb, ((p[2] & 15) * 16 + (p[0] & 15)) as usize, super::heightmap_bits(h)) + h.min_y,
            p[1] + 1
        );
        let torches = [(0, -1), (1, 0), (0, 1), (-1, 0)]
            .iter()
            .filter(|(dx, dz)| (p[0] + dx) >> 4 == col.0 && (p[2] + dz) >> 4 == col.1)
            .filter(|(dx, dz)| name([p[0] + dx, p[1], p[2] + dz]) == "minecraft:torch")
            .count();
        assert!(torches > 0, "torches around the chest");
        let sec = ((p[1] - h.min_y) >> 4) as usize + 1;
        let ly = (p[1] - h.min_y) & 15;
        let bl = c.block[sec].as_ref().expect("block light");
        let i = rc_world::block_index((p[0] & 15) as usize, ly as usize, (p[2] & 15) as usize);
        let level_at = (bl[i / 2] >> ((i & 1) * 4)) & 15;
        assert!(level_at >= 13, "chest lit by the torches: {level_at}");
        // Edge column: the torch of the neighbouring side is placed too (the game generates the neighbour).
        for (dx, dz) in [(0, -1), (1, 0), (0, 1), (-1, 0)] {
            let (tx, tz) = (p[0] + dx, p[2] + dz);
            if (tx >> 4, tz >> 4) == col {
                continue;
            }
            let loaded = dim.load(&[(tx >> 4, tz >> 4)], &data, &states);
            let crate::world::Load::Loaded(n) = &loaded[0].1 else { panic!("neighbour not full at {tx},{tz}") };
            let at = |y: i32| {
                let ly = y - h.min_y;
                n.sections[(ly >> 4) as usize].blocks.get(rc_world::block_index(
                    (tx & 15) as usize,
                    (ly & 15) as usize,
                    (tz & 15) as usize,
                ))
            };
            let supports = data.sim.get(at(p[1] - 1) as usize).is_some_and(|v| v & rc_data::sim_bits::CENTER_UP != 0);
            if supports {
                assert_eq!(data.block_of(at(p[1])).unwrap().name, "minecraft:torch", "neighbouring torch at {tx},{tz}");
            }
        }
        eprintln!("bonus chest at {p:?}, {torches} torches, light {level_at}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod disk_encode_tests {
    use rc_proto::nbt::Nbt;
    use rc_world::{Container, SECTION_BIOMES, SECTION_BLOCKS};

    use crate::lan::encode::container_tests::{biome_sections, block_sections};

    fn entry(v: u32) -> Nbt {
        Nbt::Int(v as i32)
    }

    /// Disk encoder on raw indices: same NBT as the old one, on canonical palettes or
    /// not, indices outside the palette, direct, and beyond 256 values (fallback).
    #[test]
    fn remap_writes_the_reference_nbt() {
        for c in block_sections() {
            assert_eq!(
                super::encode_container_remap(&c, SECTION_BLOCKS, true, entry),
                super::encode_container_reference(&c, SECTION_BLOCKS, true, entry),
                "{c:?}"
            );
        }
        for c in biome_sections() {
            assert_eq!(
                super::encode_container_remap(&c, SECTION_BIOMES, false, entry),
                super::encode_container_reference(&c, SECTION_BIOMES, false, entry),
                "{c:?}"
            );
        }
    }

    /// Widening at repack (`REPACK_WIDEN`): the memory palette differs, the save does not.
    #[test]
    fn widen_keeps_disk_bytes() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut r = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut differed = 0;
        for distinct in [1u32, 3, 15, 16, 60, 200] {
            let values: Vec<u32> = (0..4096u32).map(|i| (i * 7919) % distinct).collect();
            let mut a = Container::pack_reference(&values, 4, 8, 15);
            let mut b = a.clone();
            for step in 0..3000 {
                let (i, v) = (r(4096) as usize, r(u64::from(distinct) * 2 + 30) as u32);
                a.set_with(i, v, 4, 8, 15, true, false);
                b.set_with(i, v, 4, 8, 15, true, true);
                if step % 100 == 0 {
                    differed += usize::from(a != b);
                    let reference = |c: &Container| super::encode_container_reference(c, SECTION_BLOCKS, true, entry);
                    let remap = |c: &Container| super::encode_container_remap(c, SECTION_BLOCKS, true, entry);
                    assert_eq!(reference(&a), reference(&b));
                    assert_eq!(remap(&a), remap(&b));
                    assert_eq!(remap(&b), reference(&a));
                }
            }
        }
        assert!(differed > 10, "widening was almost never used: {differed}");
    }

    /// Micro-bench (`cargo test --release -p rc-sim chunk::disk_encode_tests::bench -- --ignored
    /// --nocapture`): µs per block section, old disk encoder against raw indices,
    /// 1 thread then 2, 4… up to the number of logical cores.
    #[test]
    #[ignore = "micro-bench"]
    fn bench() {
        use std::hint::black_box;
        use std::time::Instant;
        let list = block_sections();
        type Enc = fn(&Container) -> Nbt;
        let encs: [(&str, Enc); 2] = [
            ("old", |c| super::encode_container_reference(c, SECTION_BLOCKS, true, entry)),
            ("remap", |c| super::encode_container_remap(c, SECTION_BLOCKS, true, entry)),
        ];
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        for (en, f) in encs {
            let mut line = format!("disk {en:7}");
            let mut threads = 1;
            while threads <= cores {
                let reps = 20usize;
                let mut best = f64::MAX;
                for _ in 0..5 {
                    let t = Instant::now();
                    std::thread::scope(|s| {
                        for _ in 0..threads {
                            s.spawn(|| {
                                for _ in 0..reps {
                                    for c in &list {
                                        black_box(f(c));
                                    }
                                }
                            });
                        }
                    });
                    best = best.min(t.elapsed().as_secs_f64() * 1e6 / (reps * list.len() * threads) as f64);
                }
                line += &format!("  {threads}t {best:.2}");
                threads *= 2;
            }
            eprintln!("{line} µs/section ({} sections)", list.len());
        }
    }
}
