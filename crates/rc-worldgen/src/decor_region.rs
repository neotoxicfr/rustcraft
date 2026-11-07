//! Decoration region (`generation-decoration.md` § 1): read/write access to the 3×3 chunks
//! around the decorated chunk, height maps kept up to date, biomes (raw and zoomed).
//! Independent of terrain filling: one input chunk is enough.

use crate::biome::zoom;
use crate::decor_blocks::{BlockId, Blocks, Dir, State, hm};

/// Fluid types of a tick scheduled by generation (index = kind noted in
/// [`DecoChunk::fluid_ticks`]).
pub const FLUID_TICK_NAMES: [&str; 4] =
    ["minecraft:water", "minecraft:flowing_water", "minecraft:lava", "minecraft:flowing_lava"];

/// Kind of fluid tick carried by a state (water or lava, source or flowing); `None` without fluid.
#[inline]
pub fn fluid_tick_kind(b: &Blocks, s: State) -> Option<u8> {
    let flowing = u8::from(!b.is_source(s));
    match b.fluid(s) {
        1 => Some(flowing),
        2 => Some(2 + flowing),
        _ => None,
    }
}

/// Height maps, in the official order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Hm {
    WorldSurfaceWg = 0,
    WorldSurface = 1,
    OceanFloorWg = 2,
    OceanFloor = 3,
    MotionBlocking = 4,
    MotionBlockingNoLeaves = 5,
}

impl Hm {
    pub fn from_name(s: &str) -> Option<Hm> {
        Some(match s {
            "WORLD_SURFACE_WG" => Hm::WorldSurfaceWg,
            "WORLD_SURFACE" => Hm::WorldSurface,
            "OCEAN_FLOOR_WG" => Hm::OceanFloorWg,
            "OCEAN_FLOOR" => Hm::OceanFloor,
            "MOTION_BLOCKING" => Hm::MotionBlocking,
            "MOTION_BLOCKING_NO_LEAVES" => Hm::MotionBlockingNoLeaves,
            _ => return None,
        })
    }

    /// Bit of `Facts::hm` tested by this map.
    #[inline]
    pub fn bit(self) -> u8 {
        match self {
            Hm::WorldSurfaceWg | Hm::WorldSurface => hm::NOT_AIR,
            Hm::OceanFloorWg | Hm::OceanFloor => hm::MATERIAL_MOTION,
            Hm::MotionBlocking => hm::MOTION,
            Hm::MotionBlockingNoLeaves => hm::MOTION_NO_LEAVES,
        }
    }
}

/// Maps kept up to date at each placement during decoration.
pub const FINAL_HMS: [Hm; 4] = [Hm::WorldSurface, Hm::OceanFloor, Hm::MotionBlocking, Hm::MotionBlockingNoLeaves];

/// Cells of a section of 16 layers.
const SECTION_CELLS: usize = 4096;

/// Sections of terrain identifiers paired with the sections of states they convert into, from the
/// bottom (as many as `terrain` holds).
fn sections<'a>(
    terrain: &'a [u16],
    states: &'a mut [State],
) -> impl DoubleEndedIterator<Item = (&'a [u16; SECTION_CELLS], &'a mut [State; SECTION_CELLS])> + ExactSizeIterator {
    terrain.as_chunks::<SECTION_CELLS>().0.iter().zip(states.as_chunks_mut::<SECTION_CELLS>().0)
}

/// The cells (a section or a layer of terrain identifiers) hold plain air only (identifier 0). No
/// early exit: the OR over every identifier vectorises and costs less than a branch per cell.
#[inline]
fn is_terrain_air(cells: &[u16]) -> bool {
    cells.iter().fold(0, |any, &id| any | id) == 0
}

/// Search of the four final height maps layer by layer from the top: per column, the map bits not
/// found yet; the columns still searching are listed so that a layer visits only them.
struct FinalMapSearch {
    /// Heights of [`FINAL_HMS`], in its order.
    heights: [[i16; 256]; 4],
    missing: [u8; 256],
    open: [u8; 256],
    open_len: usize,
}

impl FinalMapSearch {
    fn new(bottom: i16) -> Self {
        let every_map = FINAL_HMS.iter().fold(0, |bits, m| bits | m.bit());
        FinalMapSearch {
            heights: [[bottom; 256]; 4],
            missing: [every_map; 256],
            open: std::array::from_fn(|col| col as u8),
            open_len: 256,
        }
    }

    /// Layer at world y (terrain identifiers, index `x + 16·z`), below every layer visited so far;
    /// `map_bits[id]` = height map bits of the identifier's state.
    fn visit_layer(&mut self, y: i32, layer: &[u16], map_bits: &[u8]) {
        // A layer of plain air has nothing to find: one vectorised test instead of a lookup per
        // open column (the layers above the ground of the highest sections).
        if map_bits[0] == 0 && is_terrain_air(layer) {
            return;
        }
        let mut k = 0;
        while k < self.open_len {
            let col = self.open[k] as usize;
            let hit = map_bits[layer[col] as usize] & self.missing[col];
            if hit != 0 {
                for (m, heights) in FINAL_HMS.iter().zip(&mut self.heights) {
                    if hit & m.bit() != 0 {
                        heights[col] = (y + 1) as i16;
                    }
                }
                self.missing[col] &= !hit;
                if self.missing[col] == 0 {
                    // Column done: the last open one takes its place in the list.
                    self.open_len -= 1;
                    self.open[k] = self.open[self.open_len];
                    continue;
                }
            }
            k += 1;
        }
    }
}

/// Chunk at status "terrain" (decoration input) then decorated.
#[derive(Debug, Clone)]
pub struct DecoChunk {
    pub cx: i32,
    pub cz: i32,
    pub min_y: i32,
    pub height: i32,
    /// States, index `((y − min_y)·16 + z)·16 + x`.
    pub blocks: Vec<State>,
    /// First free y per column (`x + 16·z`), per map (`Hm as usize`).
    pub heights: [[i16; 256]; 6],
    /// Raw biomes per 4×4×4 cell: index `((qy − min_y/4)·4 + qz)·4 + qx`, value = biome index.
    pub biomes: Vec<u16>,
    /// Positions marked for post-processing (arrival order).
    pub post_process: Vec<[i32; 3]>,
    /// Block entity data placed by decoration (the last one of a position wins;
    /// ignored if the final state does not carry one).
    pub be_data: Vec<([i32; 3], BeData)>,
    /// Fluid ticks scheduled during generation, with the fluid kind at the time of the
    /// tick ([`FLUID_TICK_NAMES`]). The game saves every proto-chunk tick with a delay of 0;
    /// duplicates (same kind, same position) are dropped at save.
    pub fluid_ticks: Vec<([i32; 3], u8)>,
    /// Block ticks scheduled (block targeted at the time of the tick, requested delay), same rules; the
    /// delay only matters for a placement in the live world (command, bone meal).
    pub block_ticks: Vec<([i32; 3], BlockId, i32)>,
    /// Entities created by structures (full NBT compounds, position included).
    pub entities: Vec<rc_proto::nbt::Nbt>,
    /// Disk compound `structures` (starts and references), read back or computed.
    pub structures: Option<rc_proto::nbt::Nbt>,
    /// Chunk of an old converted world to regenerate below y = 0 (`below_zero_retrogen`): the
    /// decoration centred on it only writes into the −64..−1 band.
    pub retro: Option<Retro>,
    /// Keys of the disk compound to keep as they are (`blending_data` of an old chunk).
    pub keep: Vec<(String, rc_proto::nbt::Nbt)>,
}

/// Regeneration below y = 0 of a converted chunk: columns without bedrock in the old world
/// (bits `z·16 + x`, 64-bit word per block of 64 columns), emptied over the whole band.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Retro {
    pub missing_bedrock: Vec<u64>,
}

impl Retro {
    pub fn hole(&self, x: i32, z: i32) -> bool {
        let i = ((z & 15) * 16 + (x & 15)) as usize;
        self.missing_bedrock.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 != 0)
    }
}

/// Generation band of a converted chunk (bottom, top excluded).
pub const RETRO_BAND: (i32, i32) = (-64, 0);

/// Content of a block entity set by generation.
#[derive(Debug, Clone, PartialEq)]
pub enum BeData {
    /// Container or suspicious block: loot table and its seed.
    Loot { table: &'static str, seed: i64 },
    /// Spawner: entity to spawn.
    Spawner { entity: &'static str },
    /// Beehive: bees (time already spent in the hive).
    Bees(Vec<i32>),
    /// Full data (structure templates): fields of the compound, without position.
    Raw(rc_proto::nbt::Nbt),
}

impl DecoChunk {
    pub fn new(cx: i32, cz: i32, min_y: i32, height: i32) -> Self {
        DecoChunk {
            cx,
            cz,
            min_y,
            height,
            blocks: vec![0; (16 * 16 * height) as usize],
            heights: [[min_y as i16; 256]; 6],
            biomes: vec![0; (16 * height / 4) as usize],
            post_process: Vec::new(),
            be_data: Vec::new(),
            fluid_ticks: Vec::new(),
            block_ticks: Vec::new(),
            entities: Vec::new(),
            structures: None,
            retro: None,
            keep: Vec::new(),
        }
    }

    #[inline]
    pub fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((((y - self.min_y) << 4) | (z & 15)) << 4 | (x & 15)) as usize
    }

    /// Computes the final maps from the blocks (like at the end of terrain).
    pub fn prime_final_heightmaps(&mut self, b: &Blocks) {
        self.prime_maps(b, &FINAL_HMS);
    }

    /// The given maps from the blocks: one walk per column from the highest slice that is not
    /// all air (`RUSTCRAFT_OPT_PRIME_ONE_WALK`, default), otherwise one walk per map from the top.
    pub fn prime_maps(&mut self, b: &Blocks, maps: &[Hm]) {
        if crate::opt::prime_one_walk() {
            let top = self.top_non_air(b);
            self.prime_many(b, maps, top);
        } else {
            for &hmap in maps {
                self.prime(b, hmap);
            }
        }
    }

    /// Top of the highest slice of 16 blocks that holds anything but plain air (bottom of the
    /// chunk if it is all air): no height map counts the cells above.
    pub fn top_non_air(&self, b: &Blocks) -> i32 {
        let air = b.default_state(b.k.air);
        let slices = (self.height / 16) as usize;
        (0..slices)
            .rposition(|s| self.blocks[s << 12..(s + 1) << 12].iter().any(|&v| v != air))
            .map_or(self.min_y, |s| self.min_y + (s as i32 + 1) * 16)
    }

    /// Several maps from a single walk down each column, started below `top` (every cell from
    /// `top` up is air, which no map counts): the same heights as [`Self::prime`] map by map.
    pub fn prime_many(&mut self, b: &Blocks, maps: &[Hm], top: i32) {
        let top = top.clamp(self.min_y, self.min_y + self.height);
        let want = maps.iter().fold(0u8, |a, m| a | m.bit());
        for col in 0..256usize {
            for &m in maps {
                self.heights[m as usize][col] = self.min_y as i16;
            }
            let mut left = want;
            let mut i = (((top - self.min_y) as usize) << 8) | col;
            let mut y = top;
            while left != 0 && y > self.min_y {
                i -= 256;
                y -= 1;
                let hit = b.facts(self.blocks[i]).hm & left;
                if hit != 0 {
                    for &m in maps {
                        if hit & m.bit() != 0 {
                            self.heights[m as usize][col] = (y + 1) as i16;
                        }
                    }
                    left &= !hit;
                }
            }
        }
    }

    /// Writes into the blocks, from the chunk bottom, the network state `to_state[id]` of each
    /// terrain block identifier of `terrain` (same layout), one section of 16 layers at a time. A
    /// section of plain terrain air only (identifier 0; cave air is converted like any block)
    /// becomes `air` without a lookup per cell: nothing is written when the chunk is `fresh` (all
    /// zero) and `air` is 0, the section is filled otherwise. Returns the top y of the highest
    /// section that holds anything else (the chunk bottom if none).
    pub fn convert_terrain_skipping_air(
        &mut self,
        terrain: &[u16],
        to_state: &[State],
        air: State,
        fresh: bool,
    ) -> i32 {
        let already_air = fresh && air == 0;
        let mut filled_sections = 0;
        for (k, (src, dst)) in sections(terrain, &mut self.blocks).enumerate() {
            if is_terrain_air(src) {
                if !already_air {
                    dst.fill(air);
                }
                continue;
            }
            for (d, &id) in dst.iter_mut().zip(src) {
                *d = to_state[id as usize];
            }
            filled_sections = k + 1;
        }
        self.min_y + 16 * filled_sections as i32
    }

    /// Converts `terrain` like [`Self::convert_terrain_skipping_air`] (plain air sections skipped
    /// only with `skip_air`), but layer by layer from the top, and finds the four final height maps
    /// on the way, which replaces the walk of [`Self::prime_final_heightmaps`]. `self` is a new
    /// chunk (every state 0); `map_bits[id]`: the height map bits of `to_state[id]`. Exact as long
    /// as the skipped air counts in no map.
    pub fn convert_terrain_finding_final_maps(
        &mut self,
        terrain: &[u16],
        to_state: &[State],
        map_bits: &[u8],
        air: State,
        skip_air: bool,
    ) {
        let mut search = FinalMapSearch::new(self.min_y as i16);
        for (k, (src, dst)) in sections(terrain, &mut self.blocks).enumerate().rev() {
            if skip_air && is_terrain_air(src) {
                if air != 0 {
                    dst.fill(air);
                }
                continue;
            }
            let layers = src.as_chunks::<256>().0.iter().zip(dst.as_chunks_mut::<256>().0);
            for (layer, (src, dst)) in layers.enumerate().rev() {
                for (d, &id) in dst.iter_mut().zip(src) {
                    *d = to_state[id as usize];
                }
                let y = self.min_y + 16 * k as i32 + layer as i32;
                search.visit_layer(y, src, map_bits);
            }
        }
        for (k, m) in FINAL_HMS.iter().enumerate() {
            self.heights[*m as usize] = search.heights[k];
        }
    }

    pub fn prime(&mut self, b: &Blocks, hmap: Hm) {
        let bit = hmap.bit();
        for z in 0..16 {
            for x in 0..16 {
                let mut h = self.min_y;
                for y in (self.min_y..self.min_y + self.height).rev() {
                    let s = self.blocks[self.index(x, y, z)];
                    if b.facts(s).hm & bit != 0 {
                        h = y + 1;
                        break;
                    }
                }
                self.heights[hmap as usize][(x + 16 * z) as usize] = h as i16;
            }
        }
    }
}

/// One step of [`Region::live_log`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOp {
    /// Placement with the game's flags.
    Set([i32; 3], State, u32),
    /// Neighbour update around the cell (template placed without known shape).
    Notify([i32; 3]),
    /// Break with drops (planted nether fungus): particles, sound, drops, remaining fluid.
    Destroy([i32; 3]),
    /// Direct write into the section (ore veins): no flag at all, the live world is not told.
    Raw([i32; 3], State),
}

impl LiveOp {
    /// Cell the step touches.
    pub fn pos(&self) -> [i32; 3] {
        match *self {
            LiveOp::Set(p, ..) | LiveOp::Notify(p) | LiveOp::Destroy(p) | LiveOp::Raw(p, _) => p,
        }
    }
}

/// Write area: 3×3 chunks, centre at index 4 (`(dz + 1)·3 + (dx + 1)`).
pub struct Region<'a> {
    pub chunks: [&'a mut DecoChunk; 9],
    pub b: &'a Blocks,
    pub cx: i32,
    pub cz: i32,
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    /// Bottom and generation depth (height context): max(world bottom, generator
    /// bottom) and min(world height, generator depth). Nether: 0 and 128.
    pub gen_min_y: i32,
    pub gen_depth: i32,
    /// Raw generator depth (giant mushrooms of the Nether).
    pub generator_depth: i32,
    pub seed: i64,
    pub zoom_seed: i64,
    /// Number of placements (statistic).
    pub writes: u64,
    /// Journal of the placements (debug against the probe).
    pub set_log: Option<Vec<([i32; 3], State)>>,
    /// Journal replayed into the live world (features of commands and bone meal, `place
    /// structure`, `place jigsaw`): every accepted write with its game flags, the breaks, the
    /// direct section writes and the neighbour updates, in their order.
    pub live_log: Option<Vec<LiveOp>>,
    /// Randomness of the region (factory "worldgen_region_random" at the corner of the central chunk).
    pub region_rng: crate::rng::Rng,
    /// Centre converted under regeneration: only the placements in this band (bottom, top excluded)
    /// go through, in the whole region.
    pub write_band: Option<(i32, i32)>,
}

impl<'a> Region<'a> {
    /// `chunks` in z then x increasing order (dz = −1..1, dx = −1..1).
    pub fn new(chunks: [&'a mut DecoChunk; 9], b: &'a Blocks, sea_level: i32, seed: i64) -> Self {
        let (cx, cz, min_y, height) = (chunks[4].cx, chunks[4].cz, chunks[4].min_y, chunks[4].height);
        let write_band = chunks[4].retro.as_ref().map(|_| RETRO_BAND);
        for (i, c) in chunks.iter().enumerate() {
            let (dx, dz) = (i as i32 % 3 - 1, i as i32 / 3 - 1);
            assert!(c.cx == cx + dx && c.cz == cz + dz, "region chunks out of order");
        }
        Region {
            chunks,
            b,
            cx,
            cz,
            min_y,
            height,
            sea_level,
            gen_min_y: min_y,
            gen_depth: height,
            generator_depth: height,
            seed,
            zoom_seed: crate::rng::biome_zoom_seed(seed),
            writes: 0,
            set_log: None,
            live_log: None,
            region_rng: crate::rng::Rng::xoroshiro(seed)
                .fork_positional()
                .from_hash_of("minecraft:worldgen_region_random")
                .fork_positional()
                .at(cx * 16, 0, cz * 16),
            write_band,
        }
    }

    #[inline]
    fn band_refuses(&self, y: i32) -> bool {
        self.write_band.is_some_and(|(lo, hi)| y < lo || y >= hi)
    }

    /// Sets the generator (bottom, depth): the height context follows from it.
    pub fn with_generator(mut self, gen_min_y: i32, gen_depth: i32) -> Self {
        self.gen_min_y = self.min_y.max(gen_min_y);
        self.gen_depth = self.height.min(gen_depth);
        self.generator_depth = gen_depth;
        self
    }

    /// Height context ("above the bottom", "below the top" anchors, sea level).
    #[inline]
    pub fn gen_ctx(&self) -> crate::decor_pred::GenCtx {
        crate::decor_pred::GenCtx { min_y: self.gen_min_y, depth: self.gen_depth, sea_level: self.sea_level }
    }

    #[inline]
    pub fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    #[inline]
    pub fn outside_height(&self, y: i32) -> bool {
        y < self.min_y || y >= self.min_y + self.height
    }

    #[inline]
    fn chunk_index(&self, x: i32, z: i32) -> Option<usize> {
        let dx = (x >> 4) - self.cx + 1;
        let dz = (z >> 4) - self.cz + 1;
        if (0..3).contains(&dx) && (0..3).contains(&dz) { Some((dz * 3 + dx) as usize) } else { None }
    }

    #[inline]
    pub fn in_write_zone(&self, x: i32, z: i32) -> bool {
        self.chunk_index(x, z).is_some()
    }

    /// State at (x, y, z); void air outside the height, air outside the region.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> State {
        if self.outside_height(y) {
            return self.b.default_state(self.b.k.void_air);
        }
        match self.chunk_index(x, z) {
            Some(i) => {
                let c = &self.chunks[i];
                c.blocks[c.index(x, y, z)]
            }
            None => 0,
        }
    }

    #[inline]
    pub fn get_p(&self, p: [i32; 3]) -> State {
        self.get(p[0], p[1], p[2])
    }

    /// Placement of a block by the region (game flags; only 16 "no post-processing" counts).
    pub fn set(&mut self, x: i32, y: i32, z: i32, s: State, flags: u32) -> bool {
        if let Some(l) = &mut self.set_log {
            l.push(([x, y, z], s));
        }
        let Some(i) = self.chunk_index(x, z) else { return false };
        if self.band_refuses(y) {
            return false;
        }
        if self.outside_height(y) {
            return true;
        }
        if let Some(l) = &mut self.live_log {
            l.push(LiveOp::Set([x, y, z], s, flags));
        }
        self.writes += 1;
        let b = self.b;
        let c = &mut *self.chunks[i];
        let idx = c.index(x, y, z);
        c.blocks[idx] = s;
        // Final maps.
        let col = ((x & 15) + 16 * (z & 15)) as usize;
        let bits = b.facts(s).hm;
        for hmap in FINAL_HMS {
            let first = i32::from(c.heights[hmap as usize][col]);
            if y <= first - 2 {
                continue;
            }
            if bits & hmap.bit() != 0 {
                if y >= first {
                    c.heights[hmap as usize][col] = (y + 1) as i16;
                }
            } else if first - 1 == y {
                let mut h = c.min_y;
                let base = (x & 15 | (z & 15) << 4) as usize;
                for yy in (c.min_y..y).rev() {
                    let st = c.blocks[(((yy - c.min_y) as usize) << 8) | base];
                    if b.facts(st).hm & hmap.bit() != 0 {
                        h = yy + 1;
                        break;
                    }
                }
                c.heights[hmap as usize][col] = h as i16;
            }
        }
        if flags & 16 == 0
            && let Some(dy) = post_process(b, s)
            && y + dy < c.min_y + c.height
        {
            c.post_process.push([x, y + dy, z]);
        }
        true
    }

    #[inline]
    pub fn set_p(&mut self, p: [i32; 3], s: State, flags: u32) -> bool {
        self.set(p[0], p[1], p[2], s, flags)
    }

    /// Break with drops (planted variant of the nether fungus): nothing on air; the break is
    /// journaled for the live world (which does the remaining fluid itself), and the cell keeps
    /// its fluid.
    pub fn destroy_p(&mut self, p: [i32; 3]) {
        let s = self.get_p(p);
        if self.chunk_index(p[0], p[2]).is_none()
            || self.b.is_air(s)
            || self.outside_height(p[1])
            || self.band_refuses(p[1])
        {
            return;
        }
        let b = self.b;
        let left = match b.fluid(s) {
            0 => b.default_state(b.k.air),
            // The fluid block itself (level kept), otherwise a waterlogged block: source stays.
            _ if b.is(s, b.k.water) || b.is(s, b.k.lava) => s,
            1 => b.default_state(b.k.water),
            _ => b.default_state(b.k.lava),
        };
        let log = self.live_log.take().map(|mut l| {
            l.push(LiveOp::Destroy(p));
            l
        });
        self.set_p(p, left, 3);
        self.live_log = log;
    }

    /// Barrier written without any side effect before a block that carries data (template
    /// placement, like the game): in the live world it removes the old block entity without its
    /// removal effects, and the following write always lands. Journal only: the copy is
    /// overwritten right after.
    pub fn log_barrier(&mut self, p: [i32; 3]) {
        if self.live_log.is_none()
            || self.chunk_index(p[0], p[2]).is_none()
            || self.outside_height(p[1])
            || self.band_refuses(p[1])
        {
            return;
        }
        let s = self.b.default_state(self.b.k.barrier);
        if let Some(l) = &mut self.live_log {
            l.push(LiveOp::Set(p, s, 820));
        }
    }

    /// Direct write into the section (ores): neither height maps nor post-processing.
    #[inline]
    pub fn set_direct(&mut self, x: i32, y: i32, z: i32, s: State) {
        if self.outside_height(y) || self.band_refuses(y) {
            return;
        }
        if let Some(i) = self.chunk_index(x, z) {
            if let Some(l) = &mut self.live_log {
                l.push(LiveOp::Raw([x, y, z], s));
            }
            let c = &mut *self.chunks[i];
            let idx = c.index(x, y, z);
            c.blocks[idx] = s;
            self.writes += 1;
        }
    }

    /// Chunk of the column (x, z) in the region (`None` outside the region).
    #[inline]
    pub fn column_chunk(&self, x: i32, z: i32) -> Option<usize> {
        self.chunk_index(x, z)
    }

    /// [`Self::get`] in the already resolved chunk `ci`, `y` within the height.
    #[inline]
    pub fn get_in(&self, ci: usize, x: i32, y: i32, z: i32) -> State {
        let c = &self.chunks[ci];
        c.blocks[c.index(x, y, z)]
    }

    /// [`Self::set_direct`] in the already resolved chunk `ci`, `y` within the height.
    #[inline]
    pub fn set_direct_in(&mut self, ci: usize, x: i32, y: i32, z: i32, s: State) {
        if self.band_refuses(y) {
            return;
        }
        if let Some(l) = &mut self.live_log {
            l.push(LiveOp::Raw([x, y, z], s));
        }
        let c = &mut *self.chunks[ci];
        let idx = c.index(x, y, z);
        c.blocks[idx] = s;
        self.writes += 1;
    }

    /// First free y of the column (height map read).
    #[inline]
    pub fn height_at(&self, h: Hm, x: i32, z: i32) -> i32 {
        match self.chunk_index(x, z) {
            Some(i) => i32::from(self.chunks[i].heights[h as usize][((x & 15) + 16 * (z & 15)) as usize]),
            None => self.min_y,
        }
    }

    /// Raw biome (4×4×4 cell); y clamped to the chunk height.
    pub fn noise_biome(&self, qx: i32, qy: i32, qz: i32) -> u16 {
        let Some(i) = self.chunk_index(qx << 2, qz << 2) else { return 0 };
        let c = &self.chunks[i];
        let qmin = c.min_y >> 2;
        let qy = qy.clamp(qmin, qmin + c.height / 4 - 1);
        c.biomes[(((qy - qmin) * 4 + (qz & 3)) * 4 + (qx & 3)) as usize]
    }

    /// Biome of a block (blurry zoom, like the game's biome read).
    pub fn biome(&self, x: i32, y: i32, z: i32) -> u16 {
        let (qx, qy, qz) = zoom(self.zoom_seed, x, y, z);
        self.noise_biome(qx, qy, qz)
    }

    /// A block entity exists at p (created by a placement of this decoration).
    #[inline]
    pub fn has_block_entity(&self, x: i32, y: i32, z: i32) -> bool {
        self.b.facts(self.get(x, y, z)).block_entity
    }

    #[inline]
    pub fn is_air(&self, p: [i32; 3]) -> bool {
        self.b.is_air(self.get_p(p))
    }

    /// Marks a position for post-processing (pass to `full`).
    pub fn mark_post_process(&mut self, p: [i32; 3]) {
        if self.outside_height(p[1]) {
            return;
        }
        if let Some(i) = self.chunk_index(p[0], p[2]) {
            self.chunks[i].post_process.push(p);
        }
    }

    /// Notes the content of the block entity at `p` (inside the write area only).
    pub fn note_block_entity(&mut self, p: [i32; 3], d: BeData) {
        if let Some(i) = self.chunk_index(p[0], p[2]) {
            self.chunks[i].be_data.push((p, d));
        }
    }

    /// Marks for post-processing up to two blocks above `p`, stopping at the
    /// first air (after a placed block that may remove their support).
    pub fn mark_above(&mut self, p: [i32; 3]) {
        for dy in 1..=2 {
            let q = [p[0], p[1] + dy, p[2]];
            if self.b.is_air(self.get_p(q)) {
                return;
            }
            self.mark_post_process(q);
        }
    }

    /// Schedules an immediate tick at `p` for the fluid carried by `s` (nothing without fluid; write
    /// area only).
    pub fn schedule_fluid_tick(&mut self, p: [i32; 3], s: State) {
        let Some(kind) = fluid_tick_kind(self.b, s) else { return };
        if self.outside_height(p[1]) {
            return;
        }
        if let Some(i) = self.chunk_index(p[0], p[2]) {
            self.chunks[i].fluid_ticks.push((p, kind));
        }
    }

    /// Schedules a tick of the block `block` at `p` in `delay` ticks (write area only).
    pub fn schedule_block_tick(&mut self, p: [i32; 3], block: BlockId, delay: i32) {
        if self.outside_height(p[1]) {
            return;
        }
        if let Some(i) = self.chunk_index(p[0], p[2]) {
            self.chunks[i].block_ticks.push((p, block, delay));
        }
    }

    /// Adds an entity (full compound) to the chunk that contains its block position.
    pub fn add_entity(&mut self, x: i32, z: i32, e: rc_proto::nbt::Nbt) {
        if let Some(i) = self.chunk_index(x, z) {
            self.chunks[i].entities.push(e);
        }
    }

    pub fn ensure_can_write(&self, x: i32, z: i32) -> bool {
        self.in_write_zone(x, z)
    }
}

/// Position to post-process when a state is placed (vertical offset): mushrooms
/// mark themselves, soul sand and the magma block mark the block above (bubble column
/// at the pass to `full`); other blocks, nothing.
fn post_process(b: &Blocks, s: State) -> Option<i32> {
    let k = &b.k;
    let block = b.block(s);
    if block == k.brown_mushroom || block == k.red_mushroom {
        Some(0)
    } else if block == k.soul_sand || block == k.magma_block {
        Some(1)
    } else {
        None
    }
}

/// Shape update from the neighbours (order west, east, north, south, down, up) for the
/// blocks that structure pieces mark: fences, bars and panes, torches, ladders.
fn update_shape(b: &Blocks, s: State, p: [i32; 3], get: &dyn Fn([i32; 3]) -> State) -> State {
    let name = b.block_name(b.block(s));
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    let in_tag = |st: State, t: &str| b.tag(t).is_some_and(|t| b.in_tag(st, t));
    // Fast exit (most marks: plants, full blocks): nothing to adjust.
    let attached = matches!(
        n,
        "torch"
            | "soul_torch"
            | "redstone_torch"
            | "copper_torch"
            | "wall_torch"
            | "soul_wall_torch"
            | "redstone_wall_torch"
            | "copper_wall_torch"
            | "ladder"
    );
    if !attached && !n.ends_with("fence") && !n.ends_with("bars") && !n.ends_with("glass_pane") {
        return s;
    }
    let exception = |st: State| {
        let nn = b.block_name(b.block(st));
        let nn = nn.strip_prefix("minecraft:").unwrap_or(nn);
        in_tag(st, "minecraft:leaves")
            || matches!(nn, "barrier" | "carved_pumpkin" | "jack_o_lantern" | "melon" | "pumpkin")
            || nn.ends_with("shulker_box")
    };
    let fence = in_tag(s, "minecraft:fences");
    let bars = n == "iron_bars" || n.ends_with("glass_pane") || n.ends_with("_bars");
    let air = b.default_state(b.k.air);
    let mut s = s;
    for d in [Dir::West, Dir::East, Dir::North, Dir::South, Dir::Down, Dir::Up] {
        let o = d.offset();
        let q = [p[0] + o[0], p[1] + o[1], p[2] + o[2]];
        let ns = get(q);
        if (fence || bars) && d.is_horizontal() {
            let solid = b.sturdy(ns, d.opposite());
            let connect = if fence {
                let wooden = in_tag(s, "minecraft:wooden_fences");
                let same = in_tag(ns, "minecraft:fences") && in_tag(ns, "minecraft:wooden_fences") == wooden;
                let gate = in_tag(ns, "minecraft:fence_gates")
                    && b.prop("facing")
                        .and_then(|f| b.get_dir(ns, f))
                        .is_some_and(|f| f.axis() == d.opposite().cw().axis());
                (!exception(ns) && solid) || same || gate
            } else {
                let nn = b.block_name(b.block(ns));
                let nn = nn.strip_prefix("minecraft:").unwrap_or(nn);
                let other_bars = nn == "iron_bars" || nn.ends_with("glass_pane") || nn.ends_with("_bars");
                (!exception(ns) && solid) || other_bars || in_tag(ns, "minecraft:walls")
            };
            if let Some(pr) = b.prop(d.name()) {
                s = b.with_bool(s, pr, connect);
            }
            continue;
        }
        match n {
            "torch" | "soul_torch" | "redstone_torch" | "copper_torch" if d == Dir::Down => {
                if b.facts(ns).center & (1 << Dir::Up as u8) == 0 {
                    return air;
                }
            }
            "wall_torch" | "soul_wall_torch" | "redstone_wall_torch" | "copper_wall_torch" | "ladder" => {
                if let Some(f) = b.prop("facing").and_then(|f| b.get_dir(s, f))
                    && d.opposite() == f
                    && !b.sturdy(ns, f)
                {
                    return air;
                }
            }
            _ => {}
        }
    }
    s
}

/// Post-processing at the pass to `full` (marked positions, in order): a full source
/// of water becomes a bubble column above soul sand (rising) or magma
/// (falling), and the column goes up as long as it finds sources. Fluid flow and
/// mushroom survival: not reproduced (the game does them through fluid ticks / shape
/// update).
pub fn post_process_full(c: &mut DecoChunk, b: &Blocks, outside: &dyn Fn([i32; 3]) -> State) {
    post_process_full_lit(c, b, outside, None);
}

/// Like [`post_process_full`], with the raw light level of the lit chunk (max of sky and
/// blocks): a marked mushroom that no longer holds (light level 13 or more without a block that lifts
/// the requirement, or non-full support) becomes air, like the game's shape update at the
/// pass to an active chunk.
pub fn post_process_full_lit(
    c: &mut DecoChunk,
    b: &Blocks,
    outside: &dyn Fn([i32; 3]) -> State,
    bright: Option<&dyn Fn([i32; 3]) -> u8>,
) {
    let (Some(up), Some(down)) = (
        b.state_from_str("minecraft:bubble_column[drag=false]"),
        b.state_from_str("minecraft:bubble_column[drag=true]"),
    ) else {
        return;
    };
    let bubble = b.block(up);
    let marks = std::mem::take(&mut c.post_process);
    let top = c.min_y + c.height;
    let occupy = |s: State| b.block(s) == bubble || (b.water_source(s) && b.block(s) == b.k.water);
    let mut removed = false;
    for p in &marks {
        let (x, z) = (p[0] & 15, p[2] & 15);
        let mut y = p[1];
        let s = c.blocks[c.index(x, y, z)];
        if !b.is(s, b.k.water) && !b.is(s, b.k.lava) {
            // Shapes according to the neighbours (fences, bars, torches, ladders).
            let get = |q: [i32; 3]| -> State {
                if q[1] < c.min_y || q[1] >= top {
                    return b.default_state(b.k.void_air);
                }
                if q[0] >> 4 == c.cx && q[2] >> 4 == c.cz { c.blocks[c.index(q[0], q[1], q[2])] } else { outside(q) }
            };
            let block = b.block(s);
            if let Some(light) = bright
                && (block == b.k.brown_mushroom || block == b.k.red_mushroom)
            {
                let below = get([p[0], p[1] - 1, p[2]]);
                let lifted = b.in_tag(below, b.rules.overrides_mushroom_light);
                if !lifted && (light(*p) >= 13 || !b.solid_render(below)) {
                    let i = c.index(x, y, z);
                    c.blocks[i] = b.default_state(b.k.air);
                    removed = true;
                }
                continue;
            }
            let ns = update_shape(b, s, *p, &get);
            if ns != s {
                let i = c.index(x, y, z);
                c.blocks[i] = ns;
            }
            continue;
        }
        if b.block(s) != b.k.water || !b.water_source(s) || y <= c.min_y {
            continue;
        }
        let below = c.blocks[c.index(x, y - 1, z)];
        let col = if b.block(below) == bubble {
            below
        } else if b.block(below) == b.k.soul_sand {
            up
        } else if b.block(below) == b.k.magma_block {
            down
        } else {
            continue;
        };
        while y < top && occupy(c.blocks[c.index(x, y, z)]) {
            let i = c.index(x, y, z);
            c.blocks[i] = col;
            y += 1;
        }
    }
    if removed {
        c.prime_final_heightmaps(b);
    }
    c.post_process = marks;
}

#[inline]
pub fn add(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

#[inline]
pub fn add_n(p: [i32; 3], d: Dir, n: i32) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0] * n, p[1] + o[1] * n, p[2] + o[2] * n]
}

#[inline]
pub fn offset(p: [i32; 3], dx: i32, dy: i32, dz: i32) -> [i32; 3] {
    [p[0] + dx, p[1] + dy, p[2] + dz]
}

#[cfg(test)]
mod tests {
    use super::{DecoChunk, Hm};

    /// The one-walk priming skips every slice above the highest one that holds anything but plain
    /// air, and walks the rest without testing for air: both are only right if no height map
    /// counts an air state (the game never counts plain air either, and starts below the highest
    /// section that is not all air).
    #[test]
    fn air_states_count_for_no_height_map() {
        let Some(w) = crate::structure::tests::world_data() else { return };
        let b = &*w.blocks;
        for block in [b.k.air, b.k.cave_air, b.k.void_air] {
            let s = b.default_state(block);
            assert_eq!(b.facts(s).hm, 0, "state {s} (an air) counts for a height map");
        }
        let stone = b.state_from_str("minecraft:stone").expect("stone");
        assert_ne!(b.facts(stone).hm & Hm::WorldSurface.bit(), 0, "stone must count: the check above would be empty");
    }

    /// Top of the highest slice that is not all plain air: a single block at the bottom or at the
    /// top cell of a slice moves it to the top of that slice; cave air counts as "not plain air"
    /// (harmless: it only starts the walk higher); an all-air chunk gives its bottom.
    #[test]
    fn top_non_air_is_the_top_of_the_highest_filled_slice() {
        let Some(w) = crate::structure::tests::world_data() else { return };
        let b = &*w.blocks;
        let stone = b.state_from_str("minecraft:stone").expect("stone");
        let cave_air = b.default_state(b.k.cave_air);
        let empty = || {
            let mut c = DecoChunk::new(0, 0, -64, 384);
            c.blocks.fill(b.default_state(b.k.air));
            c
        };
        assert_eq!(empty().top_non_air(b), -64, "all air");
        let cases = [(-64, stone, -48), (-49, stone, -48), (79, stone, 80), (80, stone, 96), (319, stone, 320), (200, cave_air, 208)];
        for (y, block, want) in cases {
            let mut c = empty();
            let i = c.index(5, y, 9);
            c.blocks[i] = block;
            assert_eq!(c.top_non_air(b), want, "single state {block} at y = {y}");
        }
    }

    /// Terrain identifiers for the conversion tests: 0 plain air, 1 cave air, 2 stone, 3 water,
    /// 4 leaves, 5 grass; their states are 100 + identifier, and their height map bits below.
    const STATES: [u32; 6] = [100, 101, 102, 103, 104, 105];
    const MAP_BITS: [u8; 6] = [0, 0, 1 | 2 | 4 | 8, 1 | 4 | 8, 1 | 4, 1];

    /// 24 sections drawn: every section kind (plain air, cave air only, sparse, dense) in turn,
    /// sparse cells so that columns stay open deep down.
    fn drawn_terrain(seed: i64) -> Vec<u16> {
        let mut rng = crate::decor_rng::DecoRng::legacy(seed);
        let mut terrain = vec![0u16; 24 * 4096];
        for (k, section) in terrain.as_chunks_mut::<4096>().0.iter_mut().enumerate() {
            match (k + seed as usize) % 4 {
                0 => {}
                1 => section.fill(1),
                kind => {
                    let one_in = if kind == 2 { 200 } else { 3 };
                    for id in section.iter_mut() {
                        if rng.next_int_bounded(one_in) == 0 {
                            *id = rng.next_int_bounded(STATES.len() as i32) as u16;
                        }
                    }
                }
            }
        }
        terrain
    }

    /// Converted states cell by cell (the old path).
    fn converted(terrain: &[u16]) -> Vec<u32> {
        terrain.iter().map(|&id| STATES[id as usize]).collect()
    }

    /// Skipping sections of plain terrain air gives the states of the cell by cell conversion, into
    /// a chunk holding other states (filled) as into a new one with air at 0 (left as is), and
    /// returns the top of the highest section that is not plain air: a section of cave air only is
    /// converted and counts.
    #[test]
    fn skipping_air_sections_converts_like_cell_by_cell() {
        for seed in 0..8 {
            let terrain = drawn_terrain(seed);
            let want = converted(&terrain);
            let sections = terrain.as_chunks::<4096>().0;
            let highest = sections.iter().rposition(|s| s.iter().any(|&id| id != 0)).map_or(0, |k| k + 1);
            let mut old = DecoChunk::new(0, 0, -64, 384);
            old.blocks.fill(7);
            let top = old.convert_terrain_skipping_air(&terrain, &STATES, STATES[0], false);
            assert_eq!(old.blocks, want, "seed {seed}, chunk holding other states");
            assert_eq!(top, -64 + 16 * highest as i32, "seed {seed}, ceiling");
            let zero_air: Vec<u32> = std::iter::once(0).chain(STATES[1..].iter().copied()).collect();
            let mut new = DecoChunk::new(0, 0, -64, 384);
            new.convert_terrain_skipping_air(&terrain, &zero_air, 0, true);
            let want_zero: Vec<u32> = terrain.iter().map(|&id| zero_air[id as usize]).collect();
            assert_eq!(new.blocks, want_zero, "seed {seed}, new chunk with air at 0");
        }
    }

    /// The four final maps found while converting layer by layer equal a walk down each column per
    /// map, with and without the plain air sections skipped, and the states equal the cell by cell
    /// conversion.
    #[test]
    fn final_maps_found_while_converting_match_a_walk_per_column() {
        for seed in 0..8 {
            let terrain = drawn_terrain(seed);
            for skip_air in [false, true] {
                let mut c = DecoChunk::new(0, 0, -64, 384);
                c.convert_terrain_finding_final_maps(&terrain, &STATES, &MAP_BITS, STATES[0], skip_air);
                assert_eq!(c.blocks, converted(&terrain), "seed {seed}, skip {skip_air}: states");
                for m in super::FINAL_HMS {
                    for col in 0..256 {
                        let first = (0..384).rev().find(|&y| MAP_BITS[terrain[y * 256 + col] as usize] & m.bit() != 0);
                        let want = first.map_or(-64, |y| y as i16 - 64 + 1);
                        let got = c.heights[m as usize][col];
                        assert_eq!(got, want, "seed {seed}, skip {skip_air}, {m:?}, column {col}");
                    }
                }
            }
        }
    }
}
