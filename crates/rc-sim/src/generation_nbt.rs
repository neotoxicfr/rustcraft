//! Generation chunks ↔ disk and server (`anvil-world.md` § 4, `generation-decoration.md` § 1):
//! decoration chunk (flat network states) → full server chunk or disk compound of a
//! proto-chunk (status `terrain` or decorated), and the reverse reading (the game's proto-chunks included).

use hashbrown::{HashMap, HashSet};

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_world::{Container, LightLayer, SECTION_BIOMES, SECTION_BLOCKS, Section};
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_region::{BeData, DecoChunk, FLUID_TICK_NAMES, Hm};

use crate::chunk::{self, Chunk, DATA_VERSION, HEIGHTMAPS, Height};
use crate::nbtio::{int, list};
use crate::states::States;

/// Disk status of a decorated proto-chunk (the game never writes `features`: it chains on).
pub const DECORATED: &str = "minecraft:initialize_light";
pub const TERRAIN: &str = "minecraft:terrain";

/// Rank of a disk status (official order), `None` if unknown.
pub fn status_rank(s: &str) -> Option<u8> {
    const ORDER: [&str; 12] = [
        "empty",
        "structure_starts",
        "structure_references",
        "biomes",
        "noise",
        "surface",
        "carvers",
        "terrain",
        "features",
        "initialize_light",
        "light",
        "spawn",
    ];
    let s = s.strip_prefix("minecraft:").unwrap_or(s);
    if s == "full" {
        return Some(12);
    }
    ORDER.iter().position(|o| *o == s).map(|i| i as u8)
}

pub const RANK_TERRAIN: u8 = 7;
pub const RANK_FEATURES: u8 = 8;
pub const RANK_FULL: u8 = 12;

fn bits_for(n: usize) -> u8 {
    (usize::BITS - (n.max(1) - 1).leading_zeros()) as u8
}

/// Conversions of a dimension.
pub struct Conv<'a> {
    pub data: &'a VersionData,
    pub states: &'a States,
    pub blocks: &'a Blocks,
    pub height: Height,
    /// Generator biome index → world registry identifier.
    pub biome_to_reg: Vec<u32>,
    /// Inverse (reading of proto-chunks).
    pub reg_to_biome: HashMap<u32, u16>,
    /// Block entity identifiers of the registry (name validation).
    pub be_types: Vec<String>,
}

impl<'a> Conv<'a> {
    pub fn new(
        data: &'a VersionData,
        states: &'a States,
        blocks: &'a Blocks,
        height: Height,
        biome_names: &[String],
    ) -> Self {
        let biome_to_reg: Vec<u32> =
            biome_names.iter().map(|n| states.biomes.get(n).copied().unwrap_or(states.default_biome)).collect();
        let reg_to_biome = biome_to_reg.iter().enumerate().map(|(i, &r)| (r, i as u16)).collect();
        let be_types = data.registry("minecraft:block_entity_type").map(|r| r.entries.clone()).unwrap_or_default();
        Conv { data, states, blocks, height, biome_to_reg, reg_to_biome, be_types }
    }

    fn hm_bits(&self) -> u8 {
        bits_for(self.height.sections * 16 + 1)
    }

    fn pack_heightmap(&self, v: &[i16; 256]) -> Vec<i64> {
        let bits = self.hm_bits() as usize;
        let vpl = 64 / bits;
        let mut out = vec![0i64; 256usize.div_ceil(vpl)];
        for (i, &h) in v.iter().enumerate() {
            let raw = (i32::from(h) - self.height.min_y).clamp(0, (1 << bits) - 1) as u64;
            out[i / vpl] |= (raw << ((i % vpl) * bits)) as i64;
        }
        out
    }

    fn unpack_heightmap(&self, longs: &[i64]) -> Option<[i16; 256]> {
        let bits = self.hm_bits() as usize;
        let vpl = 64 / bits;
        if longs.len() != 256usize.div_ceil(vpl) {
            return None;
        }
        let mut out = [0i16; 256];
        for (i, o) in out.iter_mut().enumerate() {
            let raw = (longs[i / vpl] as u64 >> ((i % vpl) * bits)) & ((1 << bits) - 1);
            *o = (raw as i32 + self.height.min_y) as i16;
        }
        Some(out)
    }

    fn section_blocks(&self, dc: &DecoChunk, s: usize) -> Container {
        let direct = self.data.state_bits() as u8;
        Container::pack(&dc.blocks[s * SECTION_BLOCKS..(s + 1) * SECTION_BLOCKS], 4, 8, direct)
    }

    fn section_biomes(&self, dc: &DecoChunk, s: usize) -> Vec<u32> {
        dc.biomes[s * SECTION_BIOMES..(s + 1) * SECTION_BIOMES]
            .iter()
            .map(|&b| self.biome_to_reg.get(b as usize).copied().unwrap_or(self.states.default_biome))
            .collect()
    }

    fn biomes_nbt(&self, v: &[u32]) -> Nbt {
        let mut palette: Vec<u32> = Vec::new();
        let raw: Vec<u64> = v
            .iter()
            .map(|x| match palette.iter().position(|p| p == x) {
                Some(i) => i as u64,
                None => {
                    palette.push(*x);
                    (palette.len() - 1) as u64
                }
            })
            .collect();
        let names = &self.states.biome_names;
        let pal = palette
            .iter()
            .map(|&b| Nbt::String(names.get(b as usize).cloned().unwrap_or_else(|| "minecraft:plains".into())))
            .collect();
        let mut out = vec![("palette".to_owned(), Nbt::List(pal))];
        if palette.len() > 1 {
            let b = bits_for(palette.len()) as usize;
            let vpl = 64 / b;
            let mut data = vec![0i64; v.len().div_ceil(vpl)];
            for (i, r) in raw.iter().enumerate() {
                data[i / vpl] |= (r << ((i % vpl) * b)) as i64;
            }
            out.push(("data".to_owned(), Nbt::LongArray(data)));
        }
        Nbt::Compound(out)
    }

    /// Block entities of the states that carry one, with the contents fixed by the decoration.
    /// `disk`: entities read with the chunk (kept as is where the state carries one).
    fn block_entities(&self, dc: &DecoChunk, packed: bool, disk: &[Nbt]) -> Vec<Nbt> {
        let mut extra: HashMap<[i32; 3], &BeData> = HashMap::new();
        for (p, d) in &dc.be_data {
            extra.insert(*p, d);
        }
        let mut kept: HashMap<[i32; 3], &Nbt> = HashMap::new();
        for b in disk {
            if let (Some(x), Some(y), Some(z)) = (int(b, "x"), int(b, "y"), int(b, "z")) {
                kept.insert([x as i32, y as i32, z as i32], b);
            }
        }
        let mut out = Vec::new();
        for (i, &s) in dc.blocks.iter().enumerate() {
            if !self.blocks.facts(s).block_entity {
                continue;
            }
            let name = self.blocks.block_name(self.blocks.block(s));
            let Some(id) = be_type(name) else { continue };
            if !self.be_types.contains(&id) {
                continue;
            }
            let (x, z, y) = ((i & 15) as i32, ((i >> 4) & 15) as i32, (i >> 8) as i32 + dc.min_y);
            let p = [dc.cx * 16 + x, y, dc.cz * 16 + z];
            if let Some(b) = kept.get(&p) {
                let mut b = (*b).clone();
                crate::nbtio::set(&mut b, "keepPacked", Nbt::Byte(i8::from(packed)));
                out.push(b);
                continue;
            }
            let mut f = vec![
                ("id".to_owned(), Nbt::String(id)),
                ("x".to_owned(), Nbt::Int(p[0])),
                ("y".to_owned(), Nbt::Int(p[1])),
                ("z".to_owned(), Nbt::Int(p[2])),
                ("keepPacked".to_owned(), Nbt::Byte(i8::from(packed))),
            ];
            match extra.get(&p) {
                Some(BeData::Loot { table, seed }) => {
                    f.push(("LootTable".into(), Nbt::String((*table).into())));
                    f.push(("LootTableSeed".into(), Nbt::Long(*seed)));
                }
                Some(BeData::Spawner { entity }) => {
                    for (k, v) in [
                        ("Delay", 20),
                        ("MinSpawnDelay", 200),
                        ("MaxSpawnDelay", 800),
                        ("SpawnCount", 4),
                        ("MaxNearbyEntities", 6),
                        ("RequiredPlayerRange", 16),
                        ("SpawnRange", 4),
                    ] {
                        f.push((k.into(), Nbt::Short(v)));
                    }
                    let ent = Nbt::Compound(vec![("id".into(), Nbt::String((*entity).into()))]);
                    f.push(("SpawnData".into(), Nbt::Compound(vec![("entity".into(), ent)])));
                    f.push(("SpawnPotentials".into(), Nbt::List(Vec::new())));
                }
                Some(BeData::Bees(ticks)) => {
                    let bees = ticks
                        .iter()
                        .map(|&t| {
                            let ent = Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:bee".into()))]);
                            Nbt::Compound(vec![
                                ("entity_data".into(), ent),
                                ("ticks_in_hive".into(), Nbt::Int(t)),
                                ("min_ticks_in_hive".into(), Nbt::Int(600)),
                            ])
                        })
                        .collect();
                    f.push(("bees".into(), Nbt::List(bees)));
                }
                Some(BeData::Raw(Nbt::Compound(fields))) => {
                    for (k, v) in fields {
                        if !matches!(k.as_str(), "id" | "x" | "y" | "z" | "keepPacked") {
                            f.retain(|(kk, _)| kk != k);
                            f.push((k.clone(), v.clone()));
                        }
                    }
                }
                Some(BeData::Raw(_)) | None => {}
            }
            out.push(Nbt::Compound(f));
        }
        out
    }

    fn post_processing(&self, dc: &DecoChunk) -> Nbt {
        let n = self.height.sections;
        let mut lists: Vec<Vec<Nbt>> = vec![Vec::new(); n];
        for p in &dc.post_process {
            let ly = p[1] - self.height.min_y;
            if !(0..n as i32 * 16).contains(&ly) {
                continue;
            }
            lists[(ly >> 4) as usize].push(Nbt::Short(pack_post(*p, ly)));
        }
        Nbt::List(lists.into_iter().map(Nbt::List).collect())
    }

    fn block_ticks(&self, dc: &DecoChunk) -> Nbt {
        let b = self.blocks;
        gen_ticks(dc, dc.block_ticks.iter().map(|&(p, k, _)| (p, k)), |k| Some(b.block_name(k).to_owned()))
    }

    fn base_root(&self, dc: &DecoChunk, status: &str, time: i64) -> Vec<(String, Nbt)> {
        let empty = || Nbt::Compound(Vec::new());
        let structures = dc
            .structures
            .clone()
            .unwrap_or_else(|| Nbt::Compound(vec![("References".into(), empty()), ("starts".into(), empty())]));
        vec![
            ("DataVersion".into(), Nbt::Int(DATA_VERSION)),
            ("xPos".into(), Nbt::Int(dc.cx)),
            ("yPos".into(), Nbt::Int(self.height.min_section())),
            ("zPos".into(), Nbt::Int(dc.cz)),
            ("LastUpdate".into(), Nbt::Long(time)),
            ("InhabitedTime".into(), Nbt::Long(0)),
            ("Status".into(), Nbt::String(status.into())),
            ("structures".into(), structures),
            ("fluid_ticks".into(), fluid_ticks(dc)),
            ("block_ticks".into(), self.block_ticks(dc)),
            ("PostProcessing".into(), self.post_processing(dc)),
        ]
    }

    fn heightmaps_nbt(&self, dc: &DecoChunk, maps: &[Hm]) -> Nbt {
        Nbt::Compound(
            maps.iter()
                .map(|&h| {
                    (HEIGHTMAPS[h as usize].0.to_owned(), Nbt::LongArray(self.pack_heightmap(&dc.heights[h as usize])))
                })
                .collect(),
        )
    }

    /// Disk compound of a proto-chunk (status `terrain` or decorated).
    pub fn proto_nbt(&self, dc: &DecoChunk, status: &str, time: i64, disk: &[Nbt]) -> Nbt {
        let mut root = self.base_root(dc, status, time);
        let sections = (0..self.height.sections)
            .map(|s| {
                let blocks = chunk::encode_blocks(&self.section_blocks(dc, s), self.data, self.states);
                Nbt::Compound(vec![
                    ("Y".into(), Nbt::Byte((self.height.min_section() + s as i32) as i8)),
                    ("block_states".into(), blocks),
                    ("biomes".into(), self.biomes_nbt(&self.section_biomes(dc, s))),
                ])
            })
            .collect();
        root.push(("sections".into(), Nbt::List(sections)));
        root.push(("block_entities".into(), Nbt::List(self.block_entities(dc, true, disk))));
        let all = [
            Hm::OceanFloor,
            Hm::MotionBlockingNoLeaves,
            Hm::MotionBlocking,
            Hm::WorldSurface,
            Hm::OceanFloorWg,
            Hm::WorldSurfaceWg,
        ];
        root.push(("Heightmaps".into(), self.heightmaps_nbt(dc, &all)));
        root.push(("entities".into(), Nbt::List(dc.entities.clone())));
        // Converted old chunk: blending and regeneration kept up to the full chunk.
        root.extend(dc.keep.iter().cloned());
        Nbt::Compound(root)
    }

    /// Full server chunk (status `full`), light given; to save.
    pub fn full_chunk(
        &self,
        dc: &DecoChunk,
        sky: Vec<LightLayer>,
        block: Vec<LightLayer>,
        time: i64,
        disk: &[Nbt],
    ) -> Chunk {
        let n = self.height.sections;
        let biome_direct = bits_for(self.states.biome_names.len());
        let mut sections = Vec::with_capacity(n);
        let mut nbt_sections = Vec::with_capacity(n);
        for s in 0..n {
            let blocks = self.section_blocks(dc, s);
            let biome_vals = self.section_biomes(dc, s);
            let non_empty = chunk::count_non_empty(&blocks, self.states);
            nbt_sections.push(Nbt::Compound(vec![
                ("Y".into(), Nbt::Byte((self.height.min_section() + s as i32) as i8)),
                ("block_states".into(), chunk::encode_blocks(&blocks, self.data, self.states)),
                ("biomes".into(), self.biomes_nbt(&biome_vals)),
            ]));
            let biomes = Container::pack(&biome_vals, 1, 3, biome_direct);
            sections.push(Section { non_empty, blocks, biomes });
        }
        let finals = [Hm::OceanFloor, Hm::MotionBlockingNoLeaves, Hm::MotionBlocking, Hm::WorldSurface];
        let block_entities = self.block_entities(dc, false, disk);
        let mut root = self.base_root(dc, "minecraft:full", time);
        root.push(("sections".into(), Nbt::List(nbt_sections)));
        root.push(("block_entities".into(), Nbt::List(block_entities.clone())));
        root.push(("Heightmaps".into(), self.heightmaps_nbt(dc, &finals)));
        root.push(("isLightOn".into(), Nbt::Byte(1)));
        // The full chunk keeps its old-terrain blending, plus the regeneration (finished).
        root.extend(dc.keep.iter().filter(|(k, _)| k != "below_zero_retrogen").cloned());
        let heightmaps = finals
            .iter()
            .map(|&h| (HEIGHTMAPS[h as usize].1, self.pack_heightmap(&dc.heights[h as usize]).into_boxed_slice()))
            .collect();
        Chunk {
            x: dc.cx,
            z: dc.cz,
            sections,
            sky,
            block,
            light_on: true,
            heightmaps,
            block_entities,
            inhabited_time: 0,
            root: Nbt::Compound(root),
            lossy: false,
            changed: vec![false; n],
            dirty: true,
        }
    }

    /// Reads a disk chunk at status `terrain` or higher (full ones included): decoration
    /// chunk and status rank. `None`: lower status, unreadable, or unknown identifier.
    pub fn from_nbt(&self, root: &Nbt) -> Option<(DecoChunk, u8, Vec<Nbt>)> {
        let rank = status_rank(root.get("Status")?.as_str()?)?;
        // Old chunk converted by the game (`below_zero_retrogen`): read at any status, its blocks
        // are kept and the band under y = 0 regenerated (never regenerated entirely over it).
        let retro = root.get("below_zero_retrogen").map(|r| rc_worldgen::decor_region::Retro {
            missing_bedrock: match r.get("missing_bedrock") {
                Some(Nbt::LongArray(l)) => l.iter().map(|&v| v as u64).collect(),
                Some(Nbt::List(l)) => l.iter().filter_map(|v| v.as_i64()).map(|v| v as u64).collect(),
                _ => Vec::new(),
            },
        });
        if (rank < RANK_TERRAIN && retro.is_none()) || int(root, "DataVersion")? < i64::from(DATA_VERSION) {
            return None;
        }
        let (cx, cz) = (int(root, "xPos")? as i32, int(root, "zPos")? as i32);
        let h = self.height;
        let mut dc = DecoChunk::new(cx, cz, h.min_y, h.sections as i32 * 16);
        let min = h.min_section();
        let air = self.states.air_state;
        dc.blocks.fill(air);
        // Section absent: game default biome (plains), like a new section.
        if let Some(&p) = self.states.biomes.get("minecraft:plains").and_then(|r| self.reg_to_biome.get(r)) {
            dc.biomes.fill(p);
        }
        for s in list(root.get("sections")) {
            let i = int(s, "Y")? as i32 - min;
            if !(0..h.sections as i32).contains(&i) {
                continue;
            }
            let i = i as usize;
            if let Some(bs) = s.get("block_states") {
                let pal: Vec<u32> = list(bs.get("palette"))
                    .iter()
                    .map(|e| self.states.state_of(self.data, e))
                    .collect::<Option<Vec<_>>>()?;
                let dst = &mut dc.blocks[i * SECTION_BLOCKS..(i + 1) * SECTION_BLOCKS];
                unpack(bs.get("data"), &pal, bits_for(pal.len()).max(4), dst)?;
            }
            if let Some(bi) = s.get("biomes") {
                let pal: Vec<u32> = list(bi.get("palette"))
                    .iter()
                    .map(|e| {
                        let reg = e.as_str().and_then(|n| self.states.biomes.get(n).copied())?;
                        self.reg_to_biome.get(&reg).map(|&b| u32::from(b))
                    })
                    .collect::<Option<Vec<_>>>()?;
                let mut v = [0u32; SECTION_BIOMES];
                unpack(bi.get("data"), &pal, bits_for(pal.len()), &mut v)?;
                for (d, s) in dc.biomes[i * SECTION_BIOMES..(i + 1) * SECTION_BIOMES].iter_mut().zip(v) {
                    *d = s as u16;
                }
            }
        }
        let maps = root.get("Heightmaps");
        for (k, (name, _)) in HEIGHTMAPS.iter().enumerate() {
            let hm = [
                Hm::WorldSurfaceWg,
                Hm::WorldSurface,
                Hm::OceanFloorWg,
                Hm::OceanFloor,
                Hm::MotionBlocking,
                Hm::MotionBlockingNoLeaves,
            ][k];
            match maps.and_then(|m| m.get(name)) {
                Some(Nbt::LongArray(l)) => match self.unpack_heightmap(l) {
                    Some(v) => dc.heights[hm as usize] = v,
                    None => dc.prime(self.blocks, hm),
                },
                _ => dc.prime(self.blocks, hm),
            }
        }
        for (si, l) in list(root.get("PostProcessing")).iter().enumerate() {
            let Nbt::List(l) = l else { continue };
            for v in l {
                let Nbt::Short(p) = v else { continue };
                dc.post_process.push(unpack_post(*p, cx, cz, h.min_y + si as i32 * 16));
            }
        }
        // Structures, fluid ticks and entities of the proto-chunks: kept as is.
        dc.structures = root.get("structures").cloned();
        let tick_pos = |t: &Nbt| {
            let v = |k: &str| int(t, k).map(|v| v as i32);
            Some([v("x")?, v("y")?, v("z")?])
        };
        fn tick_name(t: &Nbt) -> Option<&str> {
            t.get("i").and_then(Nbt::as_str)
        }
        for t in list(root.get("fluid_ticks")) {
            let kind = tick_name(t).and_then(|n| FLUID_TICK_NAMES.iter().position(|f| *f == n));
            if let (Some(p), Some(k)) = (tick_pos(t), kind) {
                dc.fluid_ticks.push((p, k as u8));
            }
        }
        for t in list(root.get("block_ticks")) {
            if let (Some(p), Some(k)) = (tick_pos(t), tick_name(t).and_then(|n| self.blocks.block_id(n))) {
                dc.block_ticks.push((p, k, 0));
            }
        }
        dc.entities = list(root.get("entities")).to_vec();
        dc.retro = retro;
        for k in ["blending_data", "below_zero_retrogen"] {
            if let Some(v) = root.get(k) {
                dc.keep.push((k.to_owned(), v.clone()));
            }
        }
        Some((dc, rank, list(root.get("block_entities")).to_vec()))
    }
}

/// Values of a disk palette container (`data` absent: single value).
fn unpack(data: Option<&Nbt>, pal: &[u32], bits: u8, out: &mut [u32]) -> Option<()> {
    if pal.len() == 1 || data.is_none() {
        out.fill(*pal.first()?);
        return Some(());
    }
    let Some(Nbt::LongArray(d)) = data else { return None };
    let b = bits as usize;
    let vpl = 64 / b;
    if d.len() != out.len().div_ceil(vpl) {
        return None;
    }
    let mask = (1u64 << b) - 1;
    for (i, o) in out.iter_mut().enumerate() {
        let r = ((d[i / vpl] as u64) >> ((i % vpl) * b)) & mask;
        *o = *pal.get(r as usize)?;
    }
    Some(())
}

/// Block entity type of a block (game registry names).
fn be_type(block: &str) -> Option<String> {
    let n = block.strip_prefix("minecraft:").unwrap_or(block);
    let t = match n {
        "spawner" => "mob_spawner",
        "suspicious_sand" | "suspicious_gravel" => "brushable_block",
        "bee_nest" => "beehive",
        "soul_campfire" => "campfire",
        "trapped_chest" => "trapped_chest",
        n if n.ends_with("shulker_box") => "shulker_box",
        n if n.ends_with("_hanging_sign") => "hanging_sign",
        n if n.ends_with("_sign") => "sign",
        n if n.ends_with("_banner") => "banner",
        n if n.ends_with("_skull") || n.ends_with("_head") => "skull",
        n if n.ends_with("_bed") => "bed",
        n if n.ends_with("_shelf") => "shelf",
        n if n.ends_with("copper_golem_statue") => "copper_golem_statue",
        "chain_command_block" | "repeating_command_block" => "command_block",
        "moving_piston" => "piston",
        n => n,
    };
    Some(format!("minecraft:{t}"))
}

/// Ticks scheduled by generation (`block_ticks` / `fluid_ticks`): delay 0 and normal priority
/// like any proto-chunk tick of the game (the requested delay is not kept), only one
/// per (type, position), in scheduling order; only those of the chunk.
fn gen_ticks<K: Copy + Eq + std::hash::Hash>(
    dc: &DecoChunk,
    ticks: impl Iterator<Item = ([i32; 3], K)>,
    name: impl Fn(K) -> Option<String>,
) -> Nbt {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (p, k) in ticks {
        if p[0] >> 4 != dc.cx || p[2] >> 4 != dc.cz || p[1] < dc.min_y || p[1] >= dc.min_y + dc.height {
            continue;
        }
        if !seen.insert((p, k)) {
            continue;
        }
        let Some(name) = name(k) else { continue };
        out.push(Nbt::Compound(vec![
            ("i".into(), Nbt::String(name)),
            ("x".into(), Nbt::Int(p[0])),
            ("y".into(), Nbt::Int(p[1])),
            ("z".into(), Nbt::Int(p[2])),
            ("t".into(), Nbt::Int(0)),
            ("p".into(), Nbt::Int(0)),
        ]));
    }
    Nbt::List(out)
}

fn fluid_ticks(dc: &DecoChunk) -> Nbt {
    gen_ticks(dc, dc.fluid_ticks.iter().copied(), |k| FLUID_TICK_NAMES.get(usize::from(k)).map(|n| (*n).to_owned()))
}

/// Position to post-process, game format: x | y << 4 | z << 8, local to the section (`ly`:
/// height from the bottom of the world).
fn pack_post(p: [i32; 3], ly: i32) -> i16 {
    ((p[0] & 15) | ((ly & 15) << 4) | ((p[2] & 15) << 8)) as i16
}

/// Inverse of [`pack_post`] (`y0`: bottom of the section).
fn unpack_post(v: i16, cx: i32, cz: i32, y0: i32) -> [i32; 3] {
    let v = i32::from(v);
    [cx * 16 + (v & 15), y0 + ((v >> 4) & 15), cz * 16 + ((v >> 8) & 15)]
}

#[cfg(test)]
mod post_tests {
    /// Value read in an official server world: (4, 44, 6) of chunk (1, 17), section 6
    /// of a world whose bottom is -64 → 1732.
    #[test]
    fn post_processing_offsets_use_game_layout() {
        let p = [20, 44, 278];
        assert_eq!(super::pack_post(p, 44 + 64), 1732);
        assert_eq!(super::unpack_post(1732, 1, 17, -64 + 6 * 16), p);
    }

    /// Generation ticks saved like a game proto-chunk: delay 0, priority 0, duplicates
    /// (same type, same position) discarded keeping the first, out-of-chunk ones discarded, type kept
    /// as scheduled (not read back from the final state).
    #[test]
    fn generation_fluid_ticks_saved_like_proto_chunk() {
        use rc_proto::nbt::Nbt;
        let mut dc = rc_worldgen::decor_region::DecoChunk::new(1, 2, -64, 384);
        // Blocks all at state 0 (no fluid): the old code read back the final state and
        // saved nothing.
        dc.fluid_ticks = vec![
            ([20, 70, 40], 0),  // water
            ([20, 70, 40], 0),  // duplicate
            ([20, 70, 40], 2),  // lava, same position: kept
            ([0, 70, 40], 0),   // neighbouring chunk
            ([21, 400, 41], 1), // out of height
            ([17, -64, 33], 3), // flowing lava
        ];
        let Nbt::List(l) = super::fluid_ticks(&dc) else { panic!("list expected") };
        let got: Vec<(String, i64, i64, i64, i64, i64)> = l
            .iter()
            .map(|t| {
                let i = |k: &str| t.get(k).and_then(Nbt::as_i64).expect(k);
                (t.get("i").and_then(Nbt::as_str).unwrap().to_owned(), i("x"), i("y"), i("z"), i("t"), i("p"))
            })
            .collect();
        let want =
            [("minecraft:water", 20, 70, 40), ("minecraft:lava", 20, 70, 40), ("minecraft:flowing_lava", 17, -64, 33)];
        assert_eq!(got, want.map(|(n, x, y, z)| (n.to_owned(), x, y, z, 0, 0)));
    }
}
