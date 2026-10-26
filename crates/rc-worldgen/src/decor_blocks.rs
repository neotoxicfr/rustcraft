//! Block facts useful to decoration (spec `generation-decoration.md` § 1, 4, 5): states (network
//! identifiers of rc-data), properties, block tags read from the jar, height map flags.

use hashbrown::HashMap;

use rc_data::{VersionData, flags};

use crate::Error;
use crate::decor_data::DecorData;

pub type State = u32;
pub type BlockId = u16;

/// Directions in the official order (down, up, north, south, west, east).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Dir {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

impl Dir {
    pub const ALL: [Dir; 6] = [Dir::Down, Dir::Up, Dir::North, Dir::South, Dir::West, Dir::East];
    /// Order of the horizontal directions "by 2D value": south, west, north, east.
    pub const H2D: [Dir; 4] = [Dir::South, Dir::West, Dir::North, Dir::East];
    /// Horizontals in the order north, east, south, west.
    pub const NESW: [Dir; 4] = [Dir::North, Dir::East, Dir::South, Dir::West];

    #[inline]
    pub fn offset(self) -> [i32; 3] {
        match self {
            Dir::Down => [0, -1, 0],
            Dir::Up => [0, 1, 0],
            Dir::North => [0, 0, -1],
            Dir::South => [0, 0, 1],
            Dir::West => [-1, 0, 0],
            Dir::East => [1, 0, 0],
        }
    }

    #[inline]
    pub fn opposite(self) -> Dir {
        match self {
            Dir::Down => Dir::Up,
            Dir::Up => Dir::Down,
            Dir::North => Dir::South,
            Dir::South => Dir::North,
            Dir::West => Dir::East,
            Dir::East => Dir::West,
        }
    }

    /// Clockwise rotation (seen from above); verticals unchanged.
    #[inline]
    pub fn cw(self) -> Dir {
        match self {
            Dir::North => Dir::East,
            Dir::East => Dir::South,
            Dir::South => Dir::West,
            Dir::West => Dir::North,
            d => d,
        }
    }

    #[inline]
    pub fn ccw(self) -> Dir {
        match self {
            Dir::North => Dir::West,
            Dir::West => Dir::South,
            Dir::South => Dir::East,
            Dir::East => Dir::North,
            d => d,
        }
    }

    #[inline]
    pub fn is_horizontal(self) -> bool {
        !matches!(self, Dir::Down | Dir::Up)
    }

    /// 0 = x, 1 = y, 2 = z.
    #[inline]
    pub fn axis(self) -> u8 {
        match self {
            Dir::West | Dir::East => 0,
            Dir::Down | Dir::Up => 1,
            Dir::North | Dir::South => 2,
        }
    }

    #[inline]
    pub fn positive(self) -> bool {
        matches!(self, Dir::Up | Dir::South | Dir::East)
    }

    pub fn name(self) -> &'static str {
        match self {
            Dir::Down => "down",
            Dir::Up => "up",
            Dir::North => "north",
            Dir::South => "south",
            Dir::West => "west",
            Dir::East => "east",
        }
    }

    pub fn from_name(s: &str) -> Option<Dir> {
        Some(match s {
            "down" => Dir::Down,
            "up" => Dir::Up,
            "north" => Dir::North,
            "south" => Dir::South,
            "west" => Dir::West,
            "east" => Dir::East,
            _ => return None,
        })
    }

    pub fn from_index(i: usize) -> Dir {
        Dir::ALL[i]
    }
}

pub const AXIS_NAMES: [&str; 3] = ["x", "y", "z"];

/// Bits of `Facts::hm` (height map predicates).
pub mod hm {
    pub const NOT_AIR: u8 = 1;
    pub const MATERIAL_MOTION: u8 = 2;
    pub const MOTION: u8 = 4;
    pub const MOTION_NO_LEAVES: u8 = 8;
}

/// Facts of a state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    pub block: BlockId,
    pub flags: u16,
    /// 0 none, 1 water, 2 lava.
    pub fluid: u8,
    pub fluid_amount: u8,
    pub sturdy: u8,
    pub hm: u8,
    /// Faces (bits per direction) entirely covered by the occlusion shape.
    pub occl_faces: u8,
    /// Faces entirely covered by the collision shape.
    pub coll_faces: u8,
    pub block_entity: bool,
    pub collision_empty: bool,
    pub collision_full: bool,
    /// Faces whose support shape covers the central square ("centre" support).
    pub center: u8,
    /// Faces where the collision shape is flush (non-empty surface).
    pub coll_any: u8,
    /// Faces entirely covered by the support shape.
    pub sup_faces: u8,
    /// Zones of the collision bottom face covered (walls placed below): bits 0–3 sides
    /// north, east, south, west; bit 4 central post.
    pub wall_down: u8,
}

/// Wall zones covered by the bottom face of a collision shape (sixteenths x0, x1, z0, z1).
fn wall_zones(coll: &[rc_data::Aabb]) -> u8 {
    const E: f64 = 1.0e-6;
    let mut mask = [[false; 16]; 16];
    for b in coll {
        if b.min[1] > E {
            continue;
        }
        let lo = |x: f64| ((x * 16.0) + E).floor().clamp(0.0, 16.0) as usize;
        let hi = |x: f64| ((x * 16.0) - E).ceil().clamp(0.0, 16.0) as usize;
        for row in mask.iter_mut().take(hi(b.max[0])).skip(lo(b.min[0])) {
            for c in row.iter_mut().take(hi(b.max[2])).skip(lo(b.min[2])) {
                *c = true;
            }
        }
    }
    let zones = [(7, 9, 0, 9), (7, 16, 7, 9), (7, 9, 7, 16), (0, 9, 7, 9), (7, 9, 7, 9)];
    let mut out = 0u8;
    for (i, &(x0, x1, z0, z1)) in zones.iter().enumerate() {
        if (x0..x1).all(|x| (z0..z1).all(|z| mask[x][z])) {
            out |= 1 << i;
        }
    }
    out
}

impl Facts {
    #[inline]
    pub fn coll_up_nonempty(&self) -> bool {
        self.coll_any >> Dir::Up as u8 & 1 != 0
    }
    #[inline]
    pub fn center_down(&self) -> bool {
        self.center >> Dir::Down as u8 & 1 != 0
    }
    #[inline]
    pub fn center_up(&self) -> bool {
        self.center >> Dir::Up as u8 & 1 != 0
    }
}

#[derive(Debug, Clone)]
struct PropLayout {
    name: u8,
    stride: u32,
    count: u8,
    /// Index into `Blocks::value_lists`.
    values: u16,
}

#[derive(Debug, Clone)]
struct BlockLayout {
    first: State,
    default: State,
    props: Vec<PropLayout>,
}

/// Internal identifier of a property name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Prop(pub u8);

/// Set of blocks (resolved tag), in bits per block identifier.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockSet(Vec<u64>);

impl BlockSet {
    pub fn with_capacity(blocks: usize) -> Self {
        BlockSet(vec![0; blocks.div_ceil(64)])
    }
    #[inline]
    pub fn contains(&self, b: BlockId) -> bool {
        let i = b as usize;
        self.0.get(i >> 6).is_some_and(|w| w >> (i & 63) & 1 != 0)
    }
    pub fn insert(&mut self, b: BlockId) {
        let i = b as usize;
        if self.0.len() <= i >> 6 {
            self.0.resize((i >> 6) + 1, 0);
        }
        self.0[i >> 6] |= 1 << (i & 63);
    }
    pub fn union(&mut self, o: &BlockSet) {
        if self.0.len() < o.0.len() {
            self.0.resize(o.0.len(), 0);
        }
        for (a, b) in self.0.iter_mut().zip(&o.0) {
            *a |= *b;
        }
    }
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }
}

/// Common property names, resolved once.
#[derive(Debug, Clone, Copy)]
pub struct Props {
    pub axis: Prop,
    pub distance: Prop,
    pub persistent: Prop,
    pub waterlogged: Prop,
    pub facing: Prop,
    pub half: Prop,
    pub age: Prop,
    pub snowy: Prop,
    pub north: Prop,
    pub east: Prop,
    pub south: Prop,
    pub west: Prop,
    pub up: Prop,
    pub down: Prop,
    pub tip: Prop,
    pub layers: Prop,
    pub thickness: Prop,
    pub vertical_direction: Prop,
    pub leaves: Prop,
    pub stage: Prop,
    pub level: Prop,
    pub pickles: Prop,
    pub hanging: Prop,
    pub berries: Prop,
    pub creaking_heart_state: Prop,
    pub natural: Prop,
    pub flower_amount: Prop,
    pub segment_amount: Prop,
    pub tip_direction: Prop,
    pub can_summon: Prop,
    pub honey_level: Prop,
    pub horizontal_facing: Prop,
    pub orientation: Prop,
    pub attachment: Prop,
    pub lit: Prop,
    pub dusted: Prop,
    pub bottom: Prop,
}

/// Named blocks whose behaviour is coded.
#[derive(Debug, Clone, Copy)]
pub struct Known {
    pub air: BlockId,
    pub cave_air: BlockId,
    pub void_air: BlockId,
    pub water: BlockId,
    pub lava: BlockId,
    pub stone: BlockId,
    pub cobblestone: BlockId,
    pub mossy_cobblestone: BlockId,
    pub bedrock: BlockId,
    pub dirt: BlockId,
    pub grass_block: BlockId,
    pub podzol: BlockId,
    pub mycelium: BlockId,
    pub snow: BlockId,
    pub snow_block: BlockId,
    pub ice: BlockId,
    pub packed_ice: BlockId,
    pub blue_ice: BlockId,
    pub vine: BlockId,
    pub chest: BlockId,
    pub spawner: BlockId,
    pub cocoa: BlockId,
    pub beehive: BlockId,
    pub bee_nest: BlockId,
    pub magma_block: BlockId,
    pub obsidian: BlockId,
    pub glow_lichen: BlockId,
    pub sculk_vein: BlockId,
    pub dirt_path: BlockId,
    pub bamboo: BlockId,
    pub podzol_like: BlockId,
    pub sugar_cane: BlockId,
    pub cactus: BlockId,
    pub kelp: BlockId,
    pub kelp_plant: BlockId,
    pub seagrass: BlockId,
    pub tall_seagrass: BlockId,
    pub pale_moss_carpet: BlockId,
    pub pale_hanging_moss: BlockId,
    pub creaking_heart: BlockId,
    pub mud: BlockId,
    pub muddy_mangrove_roots: BlockId,
    pub mangrove_roots: BlockId,
    pub moss_block: BlockId,
    pub farmland: BlockId,
    pub cobweb: BlockId,
    pub sand: BlockId,
    pub red_sand: BlockId,
    pub gravel: BlockId,
    pub clay: BlockId,
    pub pointed_dripstone: BlockId,
    pub dripstone_block: BlockId,
    pub sculk: BlockId,
    pub sculk_catalyst: BlockId,
    pub sculk_sensor: BlockId,
    pub sculk_shrieker: BlockId,
    pub lily_pad: BlockId,
    pub sweet_berry_bush: BlockId,
    pub cave_vines: BlockId,
    pub cave_vines_plant: BlockId,
    pub big_dripleaf: BlockId,
    pub big_dripleaf_stem: BlockId,
    pub small_dripleaf: BlockId,
    pub hanging_roots: BlockId,
    pub spore_blossom: BlockId,
    pub weeping_vines: BlockId,
    pub weeping_vines_plant: BlockId,
    pub twisting_vines: BlockId,
    pub twisting_vines_plant: BlockId,
    pub nether_wart_block: BlockId,
    pub warped_wart_block: BlockId,
    pub shroomlight: BlockId,
    pub end_stone: BlockId,
    pub chorus_plant: BlockId,
    pub chorus_flower: BlockId,
    pub torch: BlockId,
    pub wall_torch: BlockId,
    pub iron_bars: BlockId,
    pub end_portal: BlockId,
    pub end_gateway: BlockId,
    pub fire: BlockId,
    pub soul_fire: BlockId,
    pub netherrack: BlockId,
    pub soul_sand: BlockId,
    pub brown_mushroom: BlockId,
    pub red_mushroom: BlockId,
    pub soul_soil: BlockId,
    pub basalt: BlockId,
    pub magma: BlockId,
    pub sea_pickle: BlockId,
    pub leaf_litter: BlockId,
    pub moss_carpet: BlockId,
    pub firefly_bush: BlockId,
    pub dead_bush: BlockId,
    pub short_dry_grass: BlockId,
    pub tall_dry_grass: BlockId,
    pub moving_piston: BlockId,
    pub shelf_mushroom: BlockId,
}

pub struct Blocks {
    facts: Vec<Facts>,
    layouts: Vec<BlockLayout>,
    names: Vec<String>,
    by_name: HashMap<String, BlockId>,
    prop_names: Vec<String>,
    prop_by_name: HashMap<String, u8>,
    value_lists: Vec<Vec<String>>,
    /// For each list of consecutive integer values: the first one.
    int_first: Vec<Option<i32>>,
    /// Per value list: indices of `false` and `true` (`u8::MAX` if absent).
    bool_idx: Vec<[u8; 2]>,
    /// Per value list: index of the name of each direction (order of `Dir`, `u8::MAX` if absent).
    dir_idx: Vec<[u8; 6]>,
    /// Per value list: index of `x`, `y`, `z` (`u8::MAX` if absent).
    axis_idx: Vec<[u8; 3]>,
    /// Per block: bit `p` set if the block has property `p` (identifiers under 128).
    prop_mask: Vec<u128>,
    /// `RUSTCRAFT_OPT_PROP_IDX` (read at construction; the proofs switch both paths).
    pub(crate) opt_prop_idx: bool,
    tags: Vec<(BlockSet, Vec<BlockId>)>,
    tag_by_name: HashMap<String, u16>,
    fluid_tags: HashMap<String, u8>,
    pub p: Props,
    pub k: Known,
    kinds: Vec<crate::decor_rules::Kind>,
    pub rules: crate::decor_rules::RuleTags,
    pub shape: crate::shape_update::ShapeKeys,
}

impl std::fmt::Debug for Blocks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Blocks({} states, {} blocks, {} tags)", self.facts.len(), self.names.len(), self.tags.len())
    }
}

/// Boxes flush with face d; lo..hi = coverage required on the other two axes.
fn face_covers(aabbs: &[rc_data::Aabb], d: Dir, lo: f64, hi: f64, any_area: bool) -> bool {
    const E: f64 = 1e-7;
    let (a, u, v) = match d.axis() {
        0 => (0, 1, 2),
        1 => (1, 0, 2),
        _ => (2, 0, 1),
    };
    aabbs.iter().any(|b| {
        let touches = if d.positive() { b.max[a] >= 1.0 - E } else { b.min[a] <= E };
        if !touches {
            return false;
        }
        if any_area {
            return b.max[u] - b.min[u] > E && b.max[v] - b.min[v] > E;
        }
        b.min[u] <= lo + E && b.min[v] <= lo + E && b.max[u] >= hi - E && b.max[v] >= hi - E
    })
}

fn face_full(aabbs: &[rc_data::Aabb], d: Dir) -> bool {
    // A face is full if one box covers it entirely (case of all the useful shapes).
    const E: f64 = 1e-7;
    aabbs.iter().any(|b| {
        let (a, lo, hi) = match d.axis() {
            0 => (0, [1, 2], [1, 2]),
            1 => (1, [0, 2], [0, 2]),
            _ => (2, [0, 1], [0, 1]),
        };
        let touches = if d.positive() { b.max[a] >= 1.0 - E } else { b.min[a] <= E };
        touches && b.min[lo[0]] <= E && b.min[lo[1]] <= E && b.max[hi[0]] >= 1.0 - E && b.max[hi[1]] >= 1.0 - E
    })
}

/// Blocks carrying a block entity (code registry; list restricted to what decoration places
/// or the terrain contains).
const BLOCK_ENTITY_BLOCKS: &[&str] = &[
    "chest",
    "trapped_chest",
    "spawner",
    "trial_spawner",
    "vault",
    "beehive",
    "bee_nest",
    "creaking_heart",
    "sculk_sensor",
    "calibrated_sculk_sensor",
    "sculk_shrieker",
    "sculk_catalyst",
    "end_gateway",
    "end_portal",
    "barrel",
    "suspicious_sand",
    "suspicious_gravel",
    "brushable_block",
    "decorated_pot",
    "lectern",
    "furnace",
    "blast_furnace",
    "smoker",
    "brewing_stand",
    "bell",
    "campfire",
    "soul_campfire",
    "hopper",
    "dispenser",
    "dropper",
    "jukebox",
    "enchanting_table",
    "ender_chest",
    "conduit",
    "beacon",
    "comparator",
    "daylight_detector",
    "structure_block",
    "jigsaw",
    "command_block",
    "chain_command_block",
    "repeating_command_block",
    "crafter",
    "chiseled_bookshelf",
    "skeleton_skull",
    "creaking_heart",
];

impl Blocks {
    pub fn new(v: &VersionData, data: &DecorData) -> Result<Blocks, Error> {
        let mut prop_names: Vec<String> = Vec::new();
        let mut prop_by_name: HashMap<String, u8> = HashMap::new();
        let mut value_lists: Vec<Vec<String>> = Vec::new();
        let mut value_index: HashMap<Vec<String>, u16> = HashMap::new();
        let mut layouts = Vec::with_capacity(v.blocks.len());
        let mut names = Vec::with_capacity(v.blocks.len());
        let mut by_name = HashMap::new();
        for (bi, b) in v.blocks.iter().enumerate() {
            let short = b.name.strip_prefix("minecraft:").unwrap_or(&b.name).to_owned();
            by_name.insert(b.name.clone(), bi as BlockId);
            by_name.insert(short.clone(), bi as BlockId);
            names.push(short);
            // Cartesian product, last property (by name) fastest.
            let mut props = Vec::with_capacity(b.properties.len());
            let mut stride = 1u32;
            for &pi in b.properties.iter().rev() {
                let p = &v.properties[pi as usize];
                let n = prop_names.len() as u8;
                let name = *prop_by_name.entry(p.name.clone()).or_insert_with(|| {
                    prop_names.push(p.name.clone());
                    n
                });
                let vi = *value_index.entry(p.values.clone()).or_insert_with(|| {
                    value_lists.push(p.values.clone());
                    (value_lists.len() - 1) as u16
                });
                props.push(PropLayout { name, stride, count: p.values.len() as u8, values: vi });
                stride *= p.values.len() as u32;
            }
            props.reverse();
            if stride != b.state_count {
                return Err(Error::Format(format!("{}: {} states expected, {stride} computed", b.name, b.state_count)));
            }
            layouts.push(BlockLayout { first: b.first_state, default: b.default_state, props });
        }
        // Check of the state order against the value table.
        for (bi, b) in v.blocks.iter().enumerate() {
            let l = &layouts[bi];
            for s in b.first_state..b.first_state + b.state_count {
                let info = &v.states[s as usize];
                for (k, pl) in l.props.iter().enumerate() {
                    let want = v.state_values[info.values as usize + k] as u32;
                    let got = (s - l.first) / pl.stride % pl.count as u32;
                    if want != got {
                        return Err(Error::Format(format!("state order of {} inconsistent", b.name)));
                    }
                }
            }
        }
        let fluid_kind: Vec<u8> = v
            .fluids
            .iter()
            .map(|f| match f.as_str() {
                "minecraft:water" | "minecraft:flowing_water" => 1,
                "minecraft:lava" | "minecraft:flowing_lava" => 2,
                _ => 0,
            })
            .collect();
        let mut be = BlockSet::with_capacity(v.blocks.len());
        for n in BLOCK_ENTITY_BLOCKS {
            if let Some(&b) = by_name.get(*n) {
                be.insert(b);
            }
        }
        // Families (beds, signs, banners, heads, shulker boxes, shelves, statues).
        for (i, blk) in v.blocks.iter().enumerate() {
            let n = blk.name.strip_prefix("minecraft:").unwrap_or(&blk.name);
            if n.ends_with("_bed")
                || n.ends_with("_sign")
                || n.ends_with("_banner")
                || n.ends_with("_skull")
                || (n.ends_with("_head") && n != "piston_head")
                || n.ends_with("shulker_box")
                || n.ends_with("_shelf")
                || n.ends_with("copper_golem_statue")
                || n == "moving_piston"
                || n == "lectern"
            {
                be.insert(i as BlockId);
            }
        }
        // Tags.
        let mut tag_by_name = HashMap::new();
        let mut tags = Vec::new();
        let raw = data.block_tags();
        let mut resolving = Vec::new();
        for name in raw.keys() {
            resolve_tag(name, &raw, &by_name, v.blocks.len(), &mut tags, &mut tag_by_name, &mut resolving)?;
        }
        let tag = |n: &str| -> BlockSet { tag_by_name.get(n).map(|&i| tags[i as usize].0.clone()).unwrap_or_default() };
        let motion = tag("minecraft:blocks_motion_in_heightmap");
        let motion_nl = tag("minecraft:blocks_motion_in_heightmap_no_leaves");
        let mut facts = Vec::with_capacity(v.states.len());
        for s in &v.states {
            let fluid = fluid_kind.get(s.fluid as usize).copied().unwrap_or(0);
            let air = s.flags & flags::AIR != 0;
            let mut h = 0;
            if !air {
                h |= hm::NOT_AIR;
            }
            if motion.contains(s.block) {
                h |= hm::MATERIAL_MOTION;
            }
            if motion.contains(s.block) || fluid != 0 {
                h |= hm::MOTION;
            }
            if motion_nl.contains(s.block) || fluid != 0 {
                h |= hm::MOTION_NO_LEAVES;
            }
            let occl = v.shape(s.occlusion);
            let coll = v.shape(s.collision);
            let sup = v.shape(s.support);
            let mut of = 0u8;
            let mut cf = 0u8;
            let mut center = 0u8;
            let mut coll_any = 0u8;
            let mut sf = 0u8;
            for d in Dir::ALL {
                if face_full(occl, d) {
                    of |= 1 << d as u8;
                }
                if face_full(coll, d) {
                    cf |= 1 << d as u8;
                }
                if face_covers(sup, d, 7.0 / 16.0, 9.0 / 16.0, false) {
                    center |= 1 << d as u8;
                }
                if face_covers(coll, d, 0.0, 0.0, true) {
                    coll_any |= 1 << d as u8;
                }
                if face_full(sup, d) {
                    sf |= 1 << d as u8;
                }
            }
            facts.push(Facts {
                block: s.block,
                flags: s.flags,
                fluid,
                fluid_amount: s.fluid_amount,
                sturdy: s.sturdy,
                hm: h,
                occl_faces: of,
                coll_faces: cf,
                block_entity: be.contains(s.block),
                collision_empty: coll.is_empty(),
                center,
                coll_any,
                sup_faces: sf,
                collision_full: coll.len() == 1 && coll[0].min == [0.0, 0.0, 0.0] && coll[0].max == [1.0, 1.0, 1.0],
                wall_down: wall_zones(coll),
            });
        }
        let pid = |n: &str| Prop(prop_by_name.get(n).copied().unwrap_or(u8::MAX));
        let p = Props {
            axis: pid("axis"),
            distance: pid("distance"),
            persistent: pid("persistent"),
            waterlogged: pid("waterlogged"),
            facing: pid("facing"),
            half: pid("half"),
            age: pid("age"),
            snowy: pid("snowy"),
            north: pid("north"),
            east: pid("east"),
            south: pid("south"),
            west: pid("west"),
            up: pid("up"),
            down: pid("down"),
            tip: pid("tip"),
            layers: pid("layers"),
            thickness: pid("thickness"),
            vertical_direction: pid("vertical_direction"),
            leaves: pid("leaves"),
            stage: pid("stage"),
            level: pid("level"),
            pickles: pid("pickles"),
            hanging: pid("hanging"),
            berries: pid("berries"),
            creaking_heart_state: pid("creaking_heart_state"),
            natural: pid("natural"),
            flower_amount: pid("flower_amount"),
            segment_amount: pid("segment_amount"),
            tip_direction: pid("tip_direction"),
            can_summon: pid("can_summon"),
            honey_level: pid("honey_level"),
            horizontal_facing: pid("horizontal_facing"),
            orientation: pid("orientation"),
            attachment: pid("attachment"),
            lit: pid("lit"),
            dusted: pid("dusted"),
            bottom: pid("bottom"),
        };
        let bid = |n: &str| by_name.get(n).copied().unwrap_or(BlockId::MAX);
        let k = Known {
            air: bid("air"),
            cave_air: bid("cave_air"),
            void_air: bid("void_air"),
            water: bid("water"),
            lava: bid("lava"),
            stone: bid("stone"),
            cobblestone: bid("cobblestone"),
            mossy_cobblestone: bid("mossy_cobblestone"),
            bedrock: bid("bedrock"),
            dirt: bid("dirt"),
            grass_block: bid("grass_block"),
            podzol: bid("podzol"),
            mycelium: bid("mycelium"),
            snow: bid("snow"),
            snow_block: bid("snow_block"),
            ice: bid("ice"),
            packed_ice: bid("packed_ice"),
            blue_ice: bid("blue_ice"),
            vine: bid("vine"),
            chest: bid("chest"),
            spawner: bid("spawner"),
            cocoa: bid("cocoa"),
            beehive: bid("beehive"),
            bee_nest: bid("bee_nest"),
            magma_block: bid("magma_block"),
            obsidian: bid("obsidian"),
            glow_lichen: bid("glow_lichen"),
            sculk_vein: bid("sculk_vein"),
            dirt_path: bid("dirt_path"),
            bamboo: bid("bamboo"),
            podzol_like: bid("podzol"),
            sugar_cane: bid("sugar_cane"),
            cactus: bid("cactus"),
            kelp: bid("kelp"),
            kelp_plant: bid("kelp_plant"),
            seagrass: bid("seagrass"),
            tall_seagrass: bid("tall_seagrass"),
            pale_moss_carpet: bid("pale_moss_carpet"),
            pale_hanging_moss: bid("pale_hanging_moss"),
            creaking_heart: bid("creaking_heart"),
            mud: bid("mud"),
            muddy_mangrove_roots: bid("muddy_mangrove_roots"),
            mangrove_roots: bid("mangrove_roots"),
            moss_block: bid("moss_block"),
            farmland: bid("farmland"),
            cobweb: bid("cobweb"),
            sand: bid("sand"),
            red_sand: bid("red_sand"),
            gravel: bid("gravel"),
            clay: bid("clay"),
            pointed_dripstone: bid("pointed_dripstone"),
            dripstone_block: bid("dripstone_block"),
            sculk: bid("sculk"),
            sculk_catalyst: bid("sculk_catalyst"),
            sculk_sensor: bid("sculk_sensor"),
            sculk_shrieker: bid("sculk_shrieker"),
            lily_pad: bid("lily_pad"),
            sweet_berry_bush: bid("sweet_berry_bush"),
            cave_vines: bid("cave_vines"),
            cave_vines_plant: bid("cave_vines_plant"),
            big_dripleaf: bid("big_dripleaf"),
            big_dripleaf_stem: bid("big_dripleaf_stem"),
            small_dripleaf: bid("small_dripleaf"),
            hanging_roots: bid("hanging_roots"),
            spore_blossom: bid("spore_blossom"),
            weeping_vines: bid("weeping_vines"),
            weeping_vines_plant: bid("weeping_vines_plant"),
            twisting_vines: bid("twisting_vines"),
            twisting_vines_plant: bid("twisting_vines_plant"),
            nether_wart_block: bid("nether_wart_block"),
            warped_wart_block: bid("warped_wart_block"),
            shroomlight: bid("shroomlight"),
            end_stone: bid("end_stone"),
            chorus_plant: bid("chorus_plant"),
            chorus_flower: bid("chorus_flower"),
            torch: bid("torch"),
            wall_torch: bid("wall_torch"),
            iron_bars: bid("iron_bars"),
            end_portal: bid("end_portal"),
            end_gateway: bid("end_gateway"),
            fire: bid("fire"),
            soul_fire: bid("soul_fire"),
            netherrack: bid("netherrack"),
            soul_sand: bid("soul_sand"),
            brown_mushroom: bid("brown_mushroom"),
            red_mushroom: bid("red_mushroom"),
            soul_soil: bid("soul_soil"),
            basalt: bid("basalt"),
            magma: bid("magma_block"),
            sea_pickle: bid("sea_pickle"),
            leaf_litter: bid("leaf_litter"),
            moss_carpet: bid("moss_carpet"),
            firefly_bush: bid("firefly_bush"),
            dead_bush: bid("dead_bush"),
            short_dry_grass: bid("short_dry_grass"),
            tall_dry_grass: bid("tall_dry_grass"),
            moving_piston: bid("moving_piston"),
            shelf_mushroom: bid("shelf_mushroom"),
        };
        let int_first = value_lists
            .iter()
            .map(|vals| {
                let first: i32 = vals.first()?.parse().ok()?;
                vals.iter().enumerate().all(|(i, x)| x.parse::<i32>().ok() == Some(first + i as i32)).then_some(first)
            })
            .collect();
        let pos = |vals: &[String], name: &str| vals.iter().position(|x| x == name).map_or(u8::MAX, |i| i as u8);
        let bool_idx = value_lists.iter().map(|vals| [pos(vals, "false"), pos(vals, "true")]).collect();
        let dir_idx = value_lists.iter().map(|vals| Dir::ALL.map(|d| pos(vals, d.name()))).collect();
        let axis_idx = value_lists.iter().map(|vals| AXIS_NAMES.map(|a| pos(vals, a))).collect();
        let prop_mask = layouts
            .iter()
            .map(|l| l.props.iter().filter(|p| p.name < 128).fold(0u128, |m, p| m | 1 << p.name))
            .collect();
        let kinds = names.iter().map(|n| crate::decor_rules::kind_for(n)).collect();
        let mut out = Blocks {
            facts,
            layouts,
            names,
            by_name,
            prop_names,
            prop_by_name,
            value_lists,
            int_first,
            bool_idx,
            dir_idx,
            axis_idx,
            prop_mask,
            opt_prop_idx: crate::opt::prop_idx(),
            tags,
            tag_by_name,
            fluid_tags: fluid_tags(data),
            p,
            k,
            kinds,
            rules: crate::decor_rules::RuleTags::new_empty(),
            shape: crate::shape_update::ShapeKeys::empty(0),
        };
        out.rules = crate::decor_rules::RuleTags::new(&out);
        out.shape = crate::shape_update::ShapeKeys::new(&out);
        Ok(out)
    }

    #[inline]
    pub fn facts(&self, s: State) -> &Facts {
        &self.facts[s as usize]
    }

    #[inline]
    pub fn kind(&self, b: BlockId) -> crate::decor_rules::Kind {
        self.kinds[b as usize]
    }

    #[inline]
    pub fn block(&self, s: State) -> BlockId {
        self.facts[s as usize].block
    }

    #[inline]
    pub fn is(&self, s: State, b: BlockId) -> bool {
        self.facts[s as usize].block == b
    }

    pub fn block_id(&self, name: &str) -> Option<BlockId> {
        self.by_name.get(name).copied()
    }

    pub fn block_name(&self, b: BlockId) -> &str {
        &self.names[b as usize]
    }

    pub fn default_state(&self, b: BlockId) -> State {
        self.layouts[b as usize].default
    }

    pub fn state_count(&self) -> usize {
        self.facts.len()
    }

    pub fn block_count(&self) -> usize {
        self.names.len()
    }

    /// The block `b` is in the tag `t`.
    #[inline]
    pub fn tag_contains(&self, t: u16, b: BlockId) -> bool {
        self.tags[t as usize].0.contains(b)
    }

    /// Air (air, cave air, void air).
    #[inline]
    pub fn is_air(&self, s: State) -> bool {
        self.facts[s as usize].flags & flags::AIR != 0
    }

    #[inline]
    pub fn has_flag(&self, s: State, f: u16) -> bool {
        self.facts[s as usize].flags & f != 0
    }

    #[inline]
    pub fn replaceable(&self, s: State) -> bool {
        self.has_flag(s, flags::REPLACEABLE)
    }

    /// Historical "solid".
    #[inline]
    pub fn legacy_solid(&self, s: State) -> bool {
        self.has_flag(s, flags::LEGACY_SOLID)
    }

    #[inline]
    pub fn solid_render(&self, s: State) -> bool {
        self.has_flag(s, flags::SOLID_RENDER)
    }

    #[inline]
    pub fn fluid(&self, s: State) -> u8 {
        self.facts[s as usize].fluid
    }

    #[inline]
    pub fn is_source(&self, s: State) -> bool {
        self.has_flag(s, flags::FLUID_SOURCE)
    }

    /// Source water (the fluid state is a water source).
    #[inline]
    pub fn water_source(&self, s: State) -> bool {
        self.fluid(s) == 1 && self.is_source(s)
    }

    #[inline]
    pub fn sturdy(&self, s: State, d: Dir) -> bool {
        self.facts[s as usize].sturdy >> d as u8 & 1 != 0
    }

    pub fn tag(&self, name: &str) -> Option<u16> {
        crate::data::with_full_id(name.trim_start_matches('#'), |full| self.tag_by_name.get(full).copied())
    }

    /// [`Blocks::tag`] on the chosen path (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn tag_mode(&self, name: &str, borrow: bool) -> Option<u16> {
        crate::data::with_full_id_mode(name.trim_start_matches('#'), borrow, |full| self.tag_by_name.get(full).copied())
    }

    /// [`Blocks::fluid_tag`] on the chosen path (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn fluid_tag_mode(&self, name: &str, borrow: bool) -> Option<u8> {
        crate::data::with_full_id_mode(name.trim_start_matches('#'), borrow, |full| self.fluid_tags.get(full).copied())
    }

    /// Number of property names (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn prop_count(&self) -> usize {
        self.prop_names.len()
    }

    /// Names of the block and fluid tags (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn tag_names(&self) -> Vec<&str> {
        self.tag_by_name.keys().chain(self.fluid_tags.keys()).map(String::as_str).collect()
    }

    pub fn tag_set(&self, name: &str) -> BlockSet {
        self.tag(name).map(|t| self.tags[t as usize].0.clone()).unwrap_or_default()
    }

    /// Blocks of a tag, in resolution order.
    pub fn tag_list(&self, name: &str) -> Option<&[BlockId]> {
        self.tag(name).map(|t| self.tags[t as usize].1.as_slice())
    }

    /// Fluid tag: bits 1 water, 2 flowing water, 4 lava, 8 flowing lava.
    pub fn fluid_tag(&self, name: &str) -> Option<u8> {
        crate::data::with_full_id(name.trim_start_matches('#'), |full| self.fluid_tags.get(full).copied())
    }

    #[inline]
    pub fn in_tag(&self, s: State, t: u16) -> bool {
        self.tags[t as usize].0.contains(self.block(s))
    }

    // ---- Properties ----

    pub fn prop(&self, name: &str) -> Option<Prop> {
        self.prop_by_name.get(name).map(|&i| Prop(i))
    }

    #[inline]
    /// Properties of a state: (property, name, text value), in the block's order.
    pub fn props_of(&self, s: State) -> Vec<(Prop, &str, &str)> {
        let b = self.block(s);
        self.layouts[b as usize]
            .props
            .iter()
            .map(|l| {
                let p = Prop(l.name);
                let v = self.get(s, p).unwrap_or(0);
                (p, self.prop_names[l.name as usize].as_str(), self.value_lists[l.values as usize][v as usize].as_str())
            })
            .collect()
    }

    fn layout(&self, s: State, p: Prop) -> Option<&PropLayout> {
        self.layouts[self.block(s) as usize].props.iter().find(|l| l.name == p.0)
    }

    #[inline]
    pub fn has(&self, s: State, p: Prop) -> bool {
        if self.opt_prop_idx && p.0 < 128 {
            return self.prop_mask[self.block(s) as usize] >> p.0 & 1 != 0;
        }
        self.layout(s, p).is_some()
    }

    /// Value index of the property.
    #[inline]
    pub fn get(&self, s: State, p: Prop) -> Option<u8> {
        let b = self.block(s);
        let l = self.layout(s, p)?;
        Some(((s - self.layouts[b as usize].first) / l.stride % l.count as u32) as u8)
    }

    pub fn get_str(&self, s: State, p: Prop) -> Option<&str> {
        let l = self.layout(s, p)?;
        let v = self.get(s, p)?;
        Some(&self.value_lists[l.values as usize][v as usize])
    }

    #[inline]
    pub fn with(&self, s: State, p: Prop, v: u8) -> State {
        let b = self.block(s);
        match self.layout(s, p) {
            Some(l) if v < l.count => {
                let first = self.layouts[b as usize].first;
                let cur = (s - first) / l.stride % l.count as u32;
                s - cur * l.stride + v as u32 * l.stride
            }
            _ => s,
        }
    }

    pub fn value_index(&self, s: State, p: Prop, value: &str) -> Option<u8> {
        let l = self.layout(s, p)?;
        self.value_lists[l.values as usize].iter().position(|x| x == value).map(|i| i as u8)
    }

    /// Changes the property by its text value (no effect if absent).
    pub fn with_str(&self, s: State, p: Prop, value: &str) -> State {
        match self.value_index(s, p, value) {
            Some(v) => self.with(s, p, v),
            None => s,
        }
    }

    /// Layout and first value of the property in the block of `s`.
    #[inline]
    fn prop_at(&self, s: State, p: Prop) -> Option<(&PropLayout, State)> {
        let bl = &self.layouts[self.block(s) as usize];
        bl.props.iter().find(|l| l.name == p.0).map(|l| (l, bl.first))
    }

    /// Current value index, then the state where it is `v` (existing index).
    #[inline]
    fn set_index(s: State, l: &PropLayout, first: State, v: u8) -> State {
        let cur = (s - first) / l.stride % l.count as u32;
        s - cur * l.stride + v as u32 * l.stride
    }

    pub fn get_bool(&self, s: State, p: Prop) -> Option<bool> {
        if self.opt_prop_idx {
            let (l, first) = self.prop_at(s, p)?;
            let v = ((s - first) / l.stride % l.count as u32) as u8;
            return Some(v == self.bool_idx[l.values as usize][1]);
        }
        self.get_str(s, p).map(|v| v == "true")
    }

    pub fn with_bool(&self, s: State, p: Prop, b: bool) -> State {
        if self.opt_prop_idx {
            let Some((l, first)) = self.prop_at(s, p) else { return s };
            let v = self.bool_idx[l.values as usize][b as usize];
            return if v < l.count { Self::set_index(s, l, first, v) } else { s };
        }
        self.with_str(s, p, if b { "true" } else { "false" })
    }

    /// Integer value (numeric properties).
    pub fn get_int(&self, s: State, p: Prop) -> Option<i32> {
        let l = self.layout(s, p)?;
        let v = self.get(s, p)? as i32;
        match self.int_first[l.values as usize] {
            Some(first) => Some(first + v),
            None => self.get_str(s, p).and_then(|v| v.parse().ok()),
        }
    }

    pub fn with_int(&self, s: State, p: Prop, v: i32) -> State {
        let Some(l) = self.layout(s, p) else { return s };
        match self.int_first[l.values as usize] {
            Some(first) if v >= first && v - first < l.count as i32 => self.with(s, p, (v - first) as u8),
            _ => s,
        }
    }

    pub fn with_dir(&self, s: State, p: Prop, d: Dir) -> State {
        if self.opt_prop_idx {
            let Some((l, first)) = self.prop_at(s, p) else { return s };
            let v = self.dir_idx[l.values as usize][d as usize];
            return if v < l.count { Self::set_index(s, l, first, v) } else { s };
        }
        self.with_str(s, p, d.name())
    }

    /// Changes an axis property (`a`: 0 = x, 1 = y, 2 = z; no effect if absent).
    pub fn with_axis(&self, s: State, p: Prop, a: u8) -> State {
        if self.opt_prop_idx {
            let Some((l, first)) = self.prop_at(s, p) else { return s };
            let v = self.axis_idx[l.values as usize][a as usize];
            return if v < l.count { Self::set_index(s, l, first, v) } else { s };
        }
        self.with_str(s, p, AXIS_NAMES[a as usize])
    }

    pub fn get_dir(&self, s: State, p: Prop) -> Option<Dir> {
        if self.opt_prop_idx {
            let (l, first) = self.prop_at(s, p)?;
            let v = ((s - first) / l.stride % l.count as u32) as u8;
            let t = &self.dir_idx[l.values as usize];
            return Dir::ALL.into_iter().find(|&d| t[d as usize] == v);
        }
        self.get_str(s, p).and_then(Dir::from_name)
    }

    /// Copies the common properties from `from` into `to`.
    pub fn copy_props(&self, from: State, to: State) -> State {
        let mut out = to;
        let fb = self.block(from);
        for l in &self.layouts[fb as usize].props {
            let p = Prop(l.name);
            if let Some(v) = self.get_str(from, p) {
                out = self.with_str(out, p, v);
            }
        }
        out
    }

    /// State from an object `{"Name": …, "Properties": {…}}` or a bare identifier.
    pub fn parse_state(&self, j: &crate::json::Json) -> Result<State, Error> {
        use crate::json::Json;
        let (name, props) = match j {
            Json::Str(s) => (s.as_str(), None),
            _ => (
                j.get("Name")
                    .or_else(|| j.get("id"))
                    .and_then(Json::as_str)
                    .ok_or_else(|| Error::Format("state without name".into()))?,
                j.get("Properties").or_else(|| j.get("properties")),
            ),
        };
        let b = self.block_id(name).ok_or_else(|| Error::Missing(format!("block {name}")))?;
        let mut s = self.default_state(b);
        if let Some(Json::Obj(m)) = props {
            for (k, v) in m {
                let p = self.prop(k).ok_or_else(|| Error::Format(format!("property {k} of {name}")))?;
                let val = match v {
                    Json::Str(x) => x.clone(),
                    Json::Num(x) => x.clone(),
                    Json::Bool(b) => b.to_string(),
                    _ => return Err(Error::Format(format!("value of {k}"))),
                };
                let vi = self.value_index(s, p, &val).ok_or_else(|| Error::Format(format!("{name}[{k}={val}]")))?;
                s = self.with(s, p, vi);
            }
        }
        Ok(s)
    }

    /// State from its text `ns:name[p=v,…]` (form of the terrain tables). `None`: unknown block,
    /// property or value.
    pub fn state_from_str(&self, text: &str) -> Option<State> {
        let (name, props) = match text.split_once('[') {
            Some((n, rest)) => (n, rest.strip_suffix(']')?),
            None => (text, ""),
        };
        let mut s = self.default_state(self.block_id(name)?);
        for kv in props.split(',').filter(|kv| !kv.is_empty()) {
            let (k, v) = kv.split_once('=')?;
            let p = self.prop(k)?;
            s = self.with(s, p, self.value_index(s, p, v)?);
        }
        Some(s)
    }

    /// Text of a state (debug).
    pub fn describe(&self, s: State) -> String {
        let b = self.block(s);
        let mut out = self.names[b as usize].clone();
        let props = &self.layouts[b as usize].props;
        if !props.is_empty() {
            out.push('[');
            for (i, l) in props.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&self.prop_names[l.name as usize]);
                out.push('=');
                out.push_str(self.get_str(s, Prop(l.name)).unwrap_or("?"));
            }
            out.push(']');
        }
        out
    }
}

fn fluid_tags(data: &DecorData) -> HashMap<String, u8> {
    fn resolve(n: &str, raw: &HashMap<String, Vec<(String, bool)>>, depth: u32) -> u8 {
        if depth > 16 {
            return 0;
        }
        let mut m = 0;
        for (e, _) in raw.get(n).map(Vec::as_slice).unwrap_or(&[]) {
            m |= match e.as_str() {
                "minecraft:water" => 1,
                "minecraft:flowing_water" => 2,
                "minecraft:lava" => 4,
                "minecraft:flowing_lava" => 8,
                t => t.strip_prefix('#').map_or(0, |t| resolve(t, raw, depth + 1)),
            };
        }
        m
    }
    let raw = data.fluid_tags();
    raw.keys().map(|k| (k.clone(), resolve(k, &raw, 0))).collect()
}

fn resolve_tag(
    name: &str,
    raw: &HashMap<String, Vec<(String, bool)>>,
    by_name: &HashMap<String, BlockId>,
    nblocks: usize,
    tags: &mut Vec<(BlockSet, Vec<BlockId>)>,
    index: &mut HashMap<String, u16>,
    resolving: &mut Vec<String>,
) -> Result<u16, Error> {
    if let Some(&i) = index.get(name) {
        return Ok(i);
    }
    if resolving.iter().any(|n| n == name) {
        return Err(Error::Format(format!("circular tag {name}")));
    }
    let entries = raw.get(name).ok_or_else(|| Error::Missing(format!("tag {name}")))?;
    resolving.push(name.to_owned());
    let mut set = BlockSet::with_capacity(nblocks);
    let mut list = Vec::new();
    for (e, required) in entries {
        if let Some(t) = e.strip_prefix('#') {
            let t = crate::data::full_id(t);
            match resolve_tag(&t, raw, by_name, nblocks, tags, index, resolving) {
                Ok(i) => {
                    for &b in &tags[i as usize].1 {
                        if !set.contains(b) {
                            set.insert(b);
                            list.push(b);
                        }
                    }
                }
                Err(e) if *required => return Err(e),
                Err(_) => {}
            }
        } else if let Some(&b) = by_name.get(e.as_str()) {
            if !set.contains(b) {
                set.insert(b);
                list.push(b);
            }
        } else if *required {
            return Err(Error::Missing(format!("block {e} (tag {name})")));
        }
    }
    resolving.pop();
    tags.push((set, list));
    let i = (tags.len() - 1) as u16;
    index.insert(name.to_owned(), i);
    Ok(i)
}
