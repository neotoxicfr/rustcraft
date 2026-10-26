//! Random animation of blocks and fluids on the client (`particles.md` § 4).
//!
//! On every client tick (world not frozen), around the player's block: 667 iterations of two
//! draws, radius 16 then radius 32, position `center + rn(R) − rn(R)` per axis (triangular
//! distribution). For each position: animation of the drawn block (torch flames, smoke,
//! redstone dust, portal, drips, falling leaves, ambient sounds…), then of the
//! fluid and, one time in ten, a fluid drip under the block; in creative with a barrier or a
//! light block in hand, a marker at the centre of blocks of that type; if the block is not a
//! full collision cube, ambient particles of the biome (ash, spores).
//!
//! Two generators like the game: the world one (positions, drips, biome ambience and the
//! blocks that read it themselves) and the animation one (everything else). Their values are not
//! the game's (random seeds in both cases), only the distributions matter.

use rc_audio::Category;
use rc_data::VersionData;
use rc_data::model::flags;
use rc_physics::Fluid;

use crate::particles::{Kind as K, Options, Particles, TickWorld};

// ---------- Generator ----------

/// SplitMix64 with the game's draws (bounded integer, floats, boolean, gaussian).
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Self {
        Rand(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in [0, n).
    pub fn int(&mut self, n: i32) -> i32 {
        if n <= 0 {
            return 0;
        }
        (((self.next() >> 32) * n as u64) >> 32) as i32
    }

    /// Uniform integer in [a, b].
    fn between(&mut self, a: i32, b: i32) -> i32 {
        a + self.int(b - a + 1)
    }

    pub fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    pub fn d(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    fn bool(&mut self) -> bool {
        self.next() >> 63 != 0
    }

    fn gauss(&mut self) -> f64 {
        let u = self.d().max(1e-300);
        let v = self.d();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    /// Uniform double-precision float in [a, b).
    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + self.d() * (b - a)
    }
}

// ---------- Directions ----------

/// Game order: down, up, north, south, west, east (bits of `StateInfo::sturdy`).
const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
const DOWN: usize = 0;
const UP: usize = 1;
/// Horizontal plane in game order: north, east, south, west.
const HORIZONTAL: [usize; 4] = [2, 5, 3, 4];

fn opposite(d: usize) -> usize {
    d ^ 1
}

fn dir_of(name: &str) -> Option<usize> {
    Some(match name {
        "down" => 0,
        "up" => 1,
        "north" => 2,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => return None,
    })
}

fn add(p: [i32; 3], d: usize) -> [i32; 3] {
    [p[0] + DIRS[d][0], p[1] + DIRS[d][1], p[2] + DIRS[d][2]]
}

fn f3(p: [i32; 3]) -> [f64; 3] {
    p.map(f64::from)
}

// ---------- Block classification ----------

/// Own animation of a block (one entry per block of the registry).
#[derive(Clone, Copy, PartialEq, Debug)]
enum A {
    None,
    Torch(K),
    WallTorch(K),
    RedstoneTorch,
    RedstoneWallTorch,
    Fire {
        soul: bool,
    },
    Campfire {
        lava: bool,
    },
    Furnace,
    BlastFurnace,
    Smoker,
    BrewingStand,
    Candle,
    CandleCake,
    EnchantingTable,
    EnderChest,
    EndGateway,
    EndPortal,
    EndRod,
    NetherPortal,
    /// Falling block (gravel, concrete powder, anvils, dragon egg).
    Falling,
    /// Sand: dust and desert sounds.
    Sand,
    Brushable,
    /// Leaves: raindrops, sound (poplars), falling leaves.
    Leaves {
        fall: Option<(K, f32)>,
        tint: Tint,
        sound: bool,
    },
    WetSponge,
    Beehive,
    CryingObsidian,
    RespawnAnchor,
    Mycelium,
    SporeBlossom,
    BubbleColumn,
    WitherRose,
    LightningRod,
    FireflyBush,
    PointedDripstone,
    RedstoneWire,
    Repeater,
    RedstoneOre,
    Lever,
    SculkSensor,
    DriedGhast,
    DeadBush,
    DryGrass,
    Eyeblossom,
    HangingMoss,
    CreakingHeart,
    PotentSulfur,
}

/// Colour of tinted falling leaves.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Tint {
    None,
    /// Foliage colour of the biome at the position.
    Biome,
    /// Fixed colour (azaleas).
    Fixed(u32),
}

fn classify(name: &str) -> A {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    let tinted = |chance: f32| A::Leaves { fall: Some((K::TintedLeaves, chance)), tint: Tint::Biome, sound: false };
    let untinted = |k: K, chance: f32, sound: bool| A::Leaves { fall: Some((k, chance)), tint: Tint::None, sound };
    match n {
        "torch" => A::Torch(K::Flame),
        "soul_torch" => A::Torch(K::SoulFireFlame),
        "copper_torch" => A::Torch(K::CopperFireFlame),
        "wall_torch" => A::WallTorch(K::Flame),
        "soul_wall_torch" => A::WallTorch(K::SoulFireFlame),
        "copper_wall_torch" => A::WallTorch(K::CopperFireFlame),
        "redstone_torch" => A::RedstoneTorch,
        "redstone_wall_torch" => A::RedstoneWallTorch,
        "fire" => A::Fire { soul: false },
        "soul_fire" => A::Fire { soul: true },
        "campfire" => A::Campfire { lava: true },
        "soul_campfire" => A::Campfire { lava: false },
        "furnace" => A::Furnace,
        "blast_furnace" => A::BlastFurnace,
        "smoker" => A::Smoker,
        "brewing_stand" => A::BrewingStand,
        "enchanting_table" => A::EnchantingTable,
        "ender_chest" => A::EnderChest,
        "end_gateway" => A::EndGateway,
        "end_portal" => A::EndPortal,
        "end_rod" => A::EndRod,
        "nether_portal" => A::NetherPortal,
        "sand" | "red_sand" => A::Sand,
        "gravel" | "anvil" | "chipped_anvil" | "damaged_anvil" | "dragon_egg" => A::Falling,
        "suspicious_sand" | "suspicious_gravel" => A::Brushable,
        "birch_leaves" => {
            A::Leaves { fall: Some((K::TintedLeaves, 0.01)), tint: Tint::Fixed(0xff80_a755), sound: false }
        }
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "mangrove_leaves" => {
            tinted(0.01)
        }
        "spruce_leaves" => A::Leaves { fall: None, tint: Tint::None, sound: false },
        "cherry_leaves" => untinted(K::CherryLeaves, 0.1, false),
        "pale_oak_leaves" => untinted(K::PaleOakLeaves, 0.02, false),
        "red_poplar_leaves" => untinted(K::RedPoplarLeaves, 0.01, true),
        "orange_poplar_leaves" => untinted(K::OrangePoplarLeaves, 0.01, true),
        "yellow_poplar_leaves" => untinted(K::YellowPoplarLeaves, 0.01, true),
        "azalea_leaves" | "flowering_azalea_leaves" => {
            A::Leaves { fall: Some((K::TintedLeaves, 0.01)), tint: Tint::Fixed(0xff70_922d), sound: false }
        }
        "wet_sponge" => A::WetSponge,
        "beehive" | "bee_nest" => A::Beehive,
        "crying_obsidian" => A::CryingObsidian,
        "respawn_anchor" => A::RespawnAnchor,
        "mycelium" => A::Mycelium,
        "spore_blossom" => A::SporeBlossom,
        "bubble_column" => A::BubbleColumn,
        "wither_rose" => A::WitherRose,
        "firefly_bush" => A::FireflyBush,
        "pointed_dripstone" => A::PointedDripstone,
        "redstone_wire" => A::RedstoneWire,
        "repeater" => A::Repeater,
        "redstone_ore" | "deepslate_redstone_ore" => A::RedstoneOre,
        "lever" => A::Lever,
        "sculk_sensor" | "calibrated_sculk_sensor" => A::SculkSensor,
        "dried_ghast" => A::DriedGhast,
        "dead_bush" => A::DeadBush,
        "short_dry_grass" | "tall_dry_grass" => A::DryGrass,
        "open_eyeblossom" => A::Eyeblossom,
        "pale_hanging_moss" => A::HangingMoss,
        "creaking_heart" => A::CreakingHeart,
        "potent_sulfur" => A::PotentSulfur,
        "candle_cake" => A::CandleCake,
        "candle" => A::Candle,
        _ if n.ends_with("_concrete_powder") => A::Falling,
        _ if n.ends_with("_candle_cake") => A::CandleCake,
        _ if n.ends_with("_candle") => A::Candle,
        _ if n.ends_with("lightning_rod") => A::LightningRod,
        _ => A::None,
    }
}

// ---------- Block tags ----------

const T_FIRE: u16 = 1;
const T_TERRACOTTA: u16 = 1 << 1;
const T_PALE_OAK_LOGS: u16 = 1 << 2;
const T_ENCH_PROVIDER: u16 = 1 << 3;
const T_ENCH_TRANSMITTER: u16 = 1 << 4;
const T_DESERT_SAND: u16 = 1 << 5;
const T_DESERT_VEG: u16 = 1 << 6;
const T_DRIED_GHAST: u16 = 1 << 7;
const T_POPLAR_LOGS: u16 = 1 << 8;
const T_IMPERMEABLE: u16 = 1 << 9;
const T_NO_LEAVES_HEIGHT: u16 = 1 << 10;
/// Not a tag: non-zero ignite chances (fire table).
const T_FLAMMABLE: u16 = 1 << 11;

/// Tags read, in bit order.
const TAGS: [(&str, u16); 11] = [
    ("minecraft:fire", T_FIRE),
    ("minecraft:terracotta", T_TERRACOTTA),
    ("minecraft:pale_oak_logs", T_PALE_OAK_LOGS),
    ("minecraft:enchantment_power_provider", T_ENCH_PROVIDER),
    ("minecraft:enchantment_power_transmitter", T_ENCH_TRANSMITTER),
    ("minecraft:triggers_ambient_desert_sand_block_sounds", T_DESERT_SAND),
    ("minecraft:triggers_ambient_desert_dry_vegetation_block_sounds", T_DESERT_VEG),
    ("minecraft:triggers_ambient_dried_ghast_block_sounds", T_DRIED_GHAST),
    ("minecraft:required_for_poplar_leaf_ambience", T_POPLAR_LOGS),
    ("minecraft:impermeable", T_IMPERMEABLE),
    ("minecraft:blocks_motion_in_heightmap_no_leaves", T_NO_LEAVES_HEIGHT),
];

/// Contents of the base game's tags, until the server's arrive.
fn default_tags(name: &str) -> u16 {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    let mut t = 0;
    if matches!(n, "fire" | "soul_fire") {
        t |= T_FIRE;
    }
    if n == "terracotta" || n.ends_with("_terracotta") && !n.ends_with("glazed_terracotta") {
        t |= T_TERRACOTTA | T_DESERT_VEG;
    }
    if matches!(n, "pale_oak_log" | "pale_oak_wood" | "stripped_pale_oak_log" | "stripped_pale_oak_wood") {
        t |= T_PALE_OAK_LOGS;
    }
    if n == "bookshelf" {
        t |= T_ENCH_PROVIDER;
    }
    if matches!(n, "sand" | "red_sand") {
        t |= T_DESERT_SAND | T_DESERT_VEG;
    }
    if matches!(n, "soul_sand" | "soul_soil") {
        t |= T_DRIED_GHAST;
    }
    let logs = ["acacia", "birch", "oak", "jungle", "spruce", "dark_oak", "pale_oak", "mangrove", "cherry", "poplar"];
    if n.strip_suffix("_log").is_some_and(|w| logs.contains(&w)) {
        t |= T_POPLAR_LOGS;
    }
    if n == "glass" || n == "tinted_glass" || n == "barrier" || n.ends_with("_stained_glass") {
        t |= T_IMPERMEABLE;
    }
    t
}

/// Ambient particles of the biome (environment attribute of the base game's biomes).
fn biome_ambient(name: &str) -> &'static [(K, f32)] {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "soul_sand_valley" => &[(K::Ash, 0.00625)],
        "basalt_deltas" => &[(K::WhiteAsh, 0.118_093_334)],
        "crimson_forest" => &[(K::CrimsonSpore, 0.025)],
        "warped_forest" => &[(K::WarpedSpore, 0.01428)],
        _ => &[],
    }
}

/// Redstone dust colours by power (0 to 15), integers like the game.
fn wire_color(power: i32) -> u32 {
    let p = power as f32 / 15.0;
    let r = p * 0.6 + if p > 0.0 { 0.4 } else { 0.3 };
    let g = (p * p * 0.7 - 0.5).clamp(0.0, 1.0);
    let b = (p * p * 0.6 - 0.7).clamp(0.0, 1.0);
    let q = |v: f32| ((v * 255.0).floor() as u32).min(255);
    q(r) << 16 | q(g) << 8 | q(b)
}

/// Colour of ordinary redstone dust and of the torch.
const REDSTONE: u32 = 0xff_0000;
/// Starting colour of the sculk sensor's dust.
const SCULK: u32 = 0x39_d6e0;

// ---------- Context ----------

/// Sound requested by the animation: played by the caller.
pub struct Sound {
    pub name: &'static str,
    pub pos: [f64; 3],
    pub category: Category,
    pub volume: f32,
    pub pitch: f32,
}

/// World as seen by the animation, for one tick.
pub struct Ctx<'a> {
    pub w: &'a TickWorld<'a>,
    pub data: &'a VersionData,
    /// Player's block (centre of the draws) and position of their feet (player-bound sounds).
    pub player: [i32; 3],
    pub player_pos: [f64; 3],
    /// Block whose marker must be shown (creative, barrier or light in hand).
    pub marker: Option<u16>,
    /// Rain at the position (precipitation "rain", sky visible, above the map).
    pub raining_at: &'a dyn Fn([i32; 3]) -> bool,
    pub thundering: bool,
    pub game_time: i64,
    /// Night range of the day timeline (firefly bush sounds, creaking heart).
    pub night: bool,
    /// Raw maximum light at the block (`max(block, sky − darkening)`).
    pub brightness: &'a dyn Fn([i32; 3]) -> i32,
    /// Biome id at the position (received registry).
    pub biome: &'a dyn Fn([i32; 3]) -> u32,
    /// Foliage colour at the position.
    pub foliage: &'a dyn Fn([i32; 3]) -> u32,
    /// Water evaporates (Nether): dried mud, default lava drips under stalactites.
    pub evaporates: bool,
}

impl Ctx<'_> {
    fn state(&self, p: [i32; 3]) -> u32 {
        self.w.state(p[0], p[1], p[2]).unwrap_or(0)
    }

    fn info(&self, s: u32) -> rc_data::model::StateInfo {
        self.data.state(s).copied().unwrap_or_default()
    }

    fn flag(&self, s: u32, f: u16) -> bool {
        self.info(s).has(f)
    }

    fn block(&self, s: u32) -> u16 {
        self.info(s).block
    }

    fn prop(&self, s: u32, key: &str) -> Option<&str> {
        self.data.state_properties(s).find(|p| p.0 == key).map(|p| p.1)
    }

    fn is(&self, s: u32, key: &str, value: &str) -> bool {
        self.prop(s, key) == Some(value)
    }

    fn int_prop(&self, s: u32, key: &str) -> i32 {
        self.prop(s, key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }

    fn sturdy(&self, s: u32, d: usize) -> bool {
        self.info(s).sturdy & (1 << d) != 0
    }

    fn air(&self, s: u32) -> bool {
        self.w.phys.is_air(s)
    }

    fn fluid(&self, s: u32) -> Fluid {
        self.w.phys.fluids.fluid(s)
    }

    /// Boxes of the collision shape.
    fn collision(&self, s: u32) -> &[rc_data::Aabb] {
        self.data.shape(self.info(s).collision)
    }

    /// Height of a heightmap at the column (first free y), by reading the blocks.
    fn height(&self, x: i32, z: i32, test: &dyn Fn(u32) -> bool) -> i32 {
        let (lo, hi) = (self.w.min_y, self.w.min_y + self.w.height);
        if self.w.state(x, lo, z).is_none() {
            return lo;
        }
        (lo..hi).rev().find(|&y| test(self.state([x, y, z]))).map_or(lo, |y| y + 1)
    }

    /// "World surface" map: first y above the last non-air block.
    fn surface(&self, x: i32, z: i32) -> i32 {
        self.height(x, z, &|s| !self.air(s))
    }
}

fn min_max(boxes: &[rc_data::Aabb], axis: usize) -> (f64, f64) {
    if boxes.is_empty() {
        return (f64::INFINITY, f64::NEG_INFINITY);
    }
    let lo = boxes.iter().map(|b| b.min[axis]).fold(f64::INFINITY, f64::min);
    let hi = boxes.iter().map(|b| b.max[axis]).fold(f64::NEG_INFINITY, f64::max);
    (lo, hi)
}

/// Is the top face of the shape fully covered?
fn top_face_full(boxes: &[rc_data::Aabb]) -> bool {
    let area: f64 = boxes
        .iter()
        .filter(|b| b.max[1] >= 1.0 - 1e-7)
        .map(|b| (b.max[0].min(1.0) - b.min[0].max(0.0)).max(0.0) * (b.max[2].min(1.0) - b.min[2].max(0.0)).max(0.0))
        .sum();
    area >= 1.0 - 1e-7
}

// ---------- Engine ----------

pub struct BlockAnim {
    /// Animation per block.
    anim: Vec<A>,
    /// Tags per block (`T_*` bits).
    tags: Vec<u16>,
    /// Ambient particles per biome of the received registry.
    ambient: Vec<&'static [(K, f32)]>,
    /// At least one biome has ambient particles.
    any_ambient: bool,
    /// World generator and animation generator.
    world: Rand,
    anim_rng: Rand,
    /// Sounds requested since the last `take_sounds`.
    sounds: Vec<Sound>,
    /// Particular blocks (indices): air, pale moss, mud, red sand, pale oak leaves.
    air_block: u16,
    pale_moss: u16,
    mud: u16,
    red_sand: u16,
    pale_oak_leaves: u16,
    nether_portal: u16,
    /// Items that show the markers in creative (barrier, light) and their block.
    pub marker_items: Vec<(i32, u16)>,
}

impl BlockAnim {
    pub fn new(data: &VersionData) -> Self {
        let seed =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64);
        let index = |n: &str| data.blocks.iter().position(|b| b.name == n).map_or(u16::MAX, |i| i as u16);
        BlockAnim {
            anim: data.blocks.iter().map(|b| classify(&b.name)).collect(),
            tags: data
                .blocks
                .iter()
                .map(|b| {
                    let fire = if rc_sim::fire_ignite_odds(&b.name) > 0 { T_FLAMMABLE } else { 0 };
                    default_tags(&b.name) | fire
                })
                .collect(),
            ambient: Vec::new(),
            any_ambient: false,
            world: Rand::new(seed),
            anim_rng: Rand::new(seed ^ 0x000a_11ce_5eed),
            sounds: Vec::new(),
            air_block: index("minecraft:air"),
            pale_moss: index("minecraft:pale_moss_block"),
            mud: index("minecraft:mud"),
            red_sand: index("minecraft:red_sand"),
            pale_oak_leaves: index("minecraft:pale_oak_leaves"),
            nether_portal: index("minecraft:nether_portal"),
            marker_items: ["minecraft:barrier", "minecraft:light"]
                .iter()
                .filter_map(|n| {
                    let item = data.registry("minecraft:item")?.entries.iter().position(|e| e == n)?;
                    Some((item as i32, index(n)))
                })
                .collect(),
        }
    }

    /// Block tags received from the server: replace those of the base game.
    pub fn set_block_tags(&mut self, tags: &[(String, Vec<i32>)]) {
        for &(name, bit) in &TAGS {
            let Some((_, ids)) = tags.iter().find(|(n, _)| n == name) else { continue };
            for t in &mut self.tags {
                *t &= !bit;
            }
            for &i in ids {
                if let Some(t) = usize::try_from(i).ok().and_then(|i| self.tags.get_mut(i)) {
                    *t |= bit;
                }
            }
        }
    }

    /// Biome registry received (on entering a world).
    pub fn set_biomes(&mut self, names: &[String]) {
        self.ambient = names.iter().map(|n| biome_ambient(n)).collect();
        self.any_ambient = self.ambient.iter().any(|a| !a.is_empty());
    }

    pub fn take_sounds(&mut self) -> Vec<Sound> {
        std::mem::take(&mut self.sounds)
    }

    fn tag(&self, c: &Ctx<'_>, s: u32, bit: u16) -> bool {
        self.tags.get(usize::from(c.block(s))).is_some_and(|t| t & bit != 0)
    }

    fn sound(&mut self, name: &'static str, pos: [f64; 3], category: Category, volume: f32, pitch: f32) {
        self.sounds.push(Sound { name, pos, category, volume, pitch });
    }

    /// Sound at the centre of a block (the game's "block position" variant).
    fn sound_at(&mut self, name: &'static str, p: [i32; 3], category: Category, volume: f32, pitch: f32) {
        self.sound(name, f3(p).map(|v| v + 0.5), category, volume, pitch);
    }

    /// One tick: 667 × (radius 16, radius 32).
    pub fn tick(&mut self, c: &Ctx<'_>, ps: &mut Particles) {
        for _ in 0..667 {
            self.one(c, ps, 16);
            self.one(c, ps, 32);
        }
    }

    fn one(&mut self, c: &Ctx<'_>, ps: &mut Particles, r: i32) {
        let mut axis = |v: i32| v + self.world.int(r) - self.world.int(r);
        let p = [axis(c.player[0]), axis(c.player[1]), axis(c.player[2])];
        let s = c.state(p);
        let block = c.block(s);
        if let Some(&a) = self.anim.get(usize::from(block))
            && a != A::None
        {
            self.animate(c, ps, p, s, a);
        }
        let fluid = c.fluid(s);
        if fluid != Fluid::None {
            self.animate_fluid(c, ps, p, s, fluid);
            if self.world.int(10) == 0 {
                let watertight = c.sturdy(s, DOWN);
                let below = add(p, DOWN);
                let drip = if fluid == Fluid::Lava { K::DrippingLava } else { K::DrippingWater };
                self.fluid_drip(c, ps, below, c.state(below), drip, watertight);
            }
        }
        if c.marker == Some(block) {
            let o = Options { state: s, ..Options::default() };
            ps.spawn(K::BlockMarker, &o, f3(p).map(|v| v + 0.5), [0.0; 3]);
        }
        if self.any_ambient && !c.flag(s, flags::FULL_COLLISION) {
            let list = self.ambient.get((c.biome)(p) as usize).copied().unwrap_or(&[]);
            for &(k, prob) in list {
                if self.world.f() <= prob {
                    let at = [
                        f64::from(p[0]) + self.world.d(),
                        f64::from(p[1]) + self.world.d(),
                        f64::from(p[2]) + self.world.d(),
                    ];
                    ps.spawn(k, &Options::default(), at, [0.0; 3]);
                }
            }
        }
    }

    /// Fluid drip under a block that holds one (fluid above `pos`).
    fn fluid_drip(&mut self, c: &Ctx<'_>, ps: &mut Particles, pos: [i32; 3], s: u32, drip: K, top_solid: bool) {
        if c.fluid(s) != Fluid::None {
            return;
        }
        let shape = c.collision(s);
        let (bottom, top) = min_max(shape, 1);
        let y = f64::from(pos[1]);
        if top < 1.0 {
            if top_solid {
                self.drip_in(
                    ps,
                    drip,
                    [f64::from(pos[0]), f64::from(pos[0]) + 1.0],
                    [f64::from(pos[2]), f64::from(pos[2]) + 1.0],
                    y + 1.0 - 0.05,
                );
            }
        } else if !self.tag(c, s, T_IMPERMEABLE) {
            if bottom > 0.0 {
                self.drip_shape(ps, drip, pos, shape, y + bottom - 0.05);
            } else {
                let below = add(pos, DOWN);
                let bs = c.state(below);
                let (_, below_top) = min_max(c.collision(bs), 1);
                if below_top < 1.0 && c.fluid(bs) == Fluid::None {
                    self.drip_shape(ps, drip, pos, shape, y - 0.05);
                }
            }
        }
    }

    fn drip_shape(&mut self, ps: &mut Particles, drip: K, pos: [i32; 3], shape: &[rc_data::Aabb], y: f64) {
        let (x0, x1) = min_max(shape, 0);
        let (z0, z1) = min_max(shape, 2);
        let (bx, bz) = (f64::from(pos[0]), f64::from(pos[2]));
        self.drip_in(ps, drip, [bx + x0, bx + x1], [bz + z0, bz + z1], y);
    }

    fn drip_in(&mut self, ps: &mut Particles, drip: K, x: [f64; 2], z: [f64; 2], y: f64) {
        let (a, b) = (self.world.d(), self.world.d());
        let at = [x[0] + a * (x[1] - x[0]), y, z[0] + b * (z[1] - z[0])];
        ps.spawn(drip, &Options::default(), at, [0.0; 3]);
    }

    fn animate_fluid(&mut self, c: &Ctx<'_>, ps: &mut Particles, p: [i32; 3], s: u32, fluid: Fluid) {
        let r = &mut self.anim_rng;
        let b = f3(p);
        match fluid {
            Fluid::Water => {
                let source = c.w.phys.fluids.source.get(s as usize).copied().unwrap_or(false);
                let falling = c.w.phys.fluids.falling.get(s as usize).copied().unwrap_or(false);
                if !source && !falling {
                    if r.int(64) == 0 {
                        let (v, pitch) = (r.f() * 0.25 + 0.75, r.f() + 0.5);
                        self.sound_at("minecraft:block.water.ambient", p, Category::Ambient, v, pitch);
                    }
                } else if r.int(10) == 0 {
                    let at = [b[0] + r.d(), b[1] + r.d(), b[2] + r.d()];
                    ps.spawn(K::Underwater, &Options::default(), at, [0.0; 3]);
                }
            }
            Fluid::Lava => {
                let above = c.state(add(p, UP));
                if c.air(above) && !c.flag(above, flags::SOLID_RENDER) {
                    if r.int(100) == 0 {
                        let at = [b[0] + r.d(), b[1] + 1.0, b[2] + r.d()];
                        ps.spawn(K::Lava, &Options::default(), at, [0.0; 3]);
                        let (v, pitch) = (0.2 + r.f() * 0.2, 0.9 + r.f() * 0.15);
                        self.sound("minecraft:block.lava.pop", at, Category::Ambient, v, pitch);
                    }
                    let r = &mut self.anim_rng;
                    if r.int(200) == 0 {
                        let (v, pitch) = (0.2 + r.f() * 0.2, 0.9 + r.f() * 0.15);
                        self.sound("minecraft:block.lava.ambient", b, Category::Ambient, v, pitch);
                    }
                }
            }
            Fluid::None => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    fn animate(&mut self, c: &Ctx<'_>, ps: &mut Particles, p: [i32; 3], s: u32, a: A) {
        let b = f3(p);
        let none = [0.0; 3];
        let o = Options::default();
        let facing = || c.prop(s, "facing").and_then(dir_of).unwrap_or(2);
        let lit = || c.is(s, "lit", "true");
        match a {
            A::None => {}
            A::Torch(flame) => {
                let at = [b[0] + 0.5, b[1] + 0.7, b[2] + 0.5];
                ps.spawn(K::Smoke, &o, at, none);
                ps.spawn(flame, &o, at, none);
            }
            A::WallTorch(flame) => {
                let d = DIRS[opposite(facing())];
                let at = [b[0] + 0.5 + 0.27 * f64::from(d[0]), b[1] + 0.7 + 0.22, b[2] + 0.5 + 0.27 * f64::from(d[2])];
                ps.spawn(K::Smoke, &o, at, none);
                ps.spawn(flame, &o, at, none);
            }
            A::RedstoneTorch | A::RedstoneWallTorch => {
                if !lit() {
                    return;
                }
                let r = &mut self.anim_rng;
                let mut at = [
                    b[0] + 0.5 + (r.d() - 0.5) * 0.2,
                    b[1] + 0.7 + (r.d() - 0.5) * 0.2,
                    b[2] + 0.5 + (r.d() - 0.5) * 0.2,
                ];
                if a == A::RedstoneWallTorch {
                    let d = DIRS[opposite(facing())];
                    at = [at[0] + 0.27 * f64::from(d[0]), at[1] + 0.22, at[2] + 0.27 * f64::from(d[2])];
                }
                ps.spawn(K::Dust, &Options::dust(REDSTONE, 1.0), at, none);
            }
            A::Fire { soul } => self.fire(c, ps, p, soul),
            A::Campfire { lava } => {
                if !lit() {
                    return;
                }
                let r = &mut self.anim_rng;
                if r.int(10) == 0 {
                    let (v, pitch) = (0.5 + r.f(), r.f() * 0.7 + 0.6);
                    self.sound_at("minecraft:block.campfire.crackle", p, Category::Block, v, pitch);
                }
                let r = &mut self.anim_rng;
                if lava && r.int(5) == 0 {
                    for _ in 0..r.int(1) + 1 {
                        let v = [f64::from(r.f() / 2.0), 5.0e-5, f64::from(r.f() / 2.0)];
                        ps.spawn(K::Lava, &o, b.map(|v| v + 0.5), v);
                    }
                }
            }
            A::Furnace | A::BlastFurnace => {
                if !lit() {
                    return;
                }
                let (x, y, z) = (b[0] + 0.5, b[1], b[2] + 0.5);
                let r = &mut self.anim_rng;
                if r.d() < 0.1 {
                    let name = if a == A::Furnace {
                        "minecraft:block.furnace.fire_crackle"
                    } else {
                        "minecraft:block.blastfurnace.fire_crackle"
                    };
                    self.sound(name, [x, y, z], Category::Block, 1.0, 1.0);
                }
                let r = &mut self.anim_rng;
                let d = facing();
                let ss = r.d() * 0.6 - 0.3;
                let dx = if DIRS[d][0] != 0 { f64::from(DIRS[d][0]) * 0.52 } else { ss };
                let dy = r.d() * if a == A::Furnace { 6.0 } else { 9.0 } / 16.0;
                let dz = if DIRS[d][2] != 0 { f64::from(DIRS[d][2]) * 0.52 } else { ss };
                let at = [x + dx, y + dy, z + dz];
                ps.spawn(K::Smoke, &o, at, none);
                if a == A::Furnace {
                    ps.spawn(K::Flame, &o, at, none);
                }
            }
            A::Smoker => {
                if !lit() {
                    return;
                }
                let (x, y, z) = (b[0] + 0.5, b[1], b[2] + 0.5);
                if self.anim_rng.d() < 0.1 {
                    self.sound("minecraft:block.smoker.smoke", [x, y, z], Category::Block, 1.0, 1.0);
                }
                ps.spawn(K::Smoke, &o, [x, y + 1.1, z], none);
            }
            A::BrewingStand => {
                let r = &mut self.anim_rng;
                let at = [
                    b[0] + 0.4 + f64::from(r.f()) * 0.2,
                    b[1] + 0.7 + f64::from(r.f()) * 0.3,
                    b[2] + 0.4 + f64::from(r.f()) * 0.2,
                ];
                ps.spawn(K::Smoke, &o, at, none);
            }
            A::Candle | A::CandleCake => {
                if !lit() {
                    return;
                }
                const ONE: &[[f64; 3]] = &[[8.0, 8.0, 8.0]];
                const TWO: &[[f64; 3]] = &[[6.0, 7.0, 8.0], [10.0, 8.0, 7.0]];
                const THREE: &[[f64; 3]] = &[[8.0, 5.0, 10.0], [6.0, 7.0, 8.0], [9.0, 8.0, 7.0]];
                const FOUR: &[[f64; 3]] = &[[7.0, 5.0, 9.0], [10.0, 7.0, 9.0], [6.0, 7.0, 6.0], [9.0, 8.0, 6.0]];
                const CAKE: &[[f64; 3]] = &[[8.0, 16.0, 8.0]];
                let offsets = if a == A::CandleCake {
                    CAKE
                } else {
                    match c.int_prop(s, "candles") {
                        2 => TWO,
                        3 => THREE,
                        4 => FOUR,
                        _ => ONE,
                    }
                };
                for off in offsets {
                    let at = [b[0] + off[0] * 0.0625, b[1] + off[1] * 0.0625, b[2] + off[2] * 0.0625];
                    let r = &mut self.anim_rng;
                    let chance = r.f();
                    if chance < 0.3 {
                        ps.spawn(K::Smoke, &o, at, none);
                        if chance < 0.17 {
                            let (v, pitch) = (1.0 + r.f(), r.f() * 0.7 + 0.3);
                            self.sound(
                                "minecraft:block.candle.ambient",
                                at.map(|v| v + 0.5),
                                Category::Block,
                                v,
                                pitch,
                            );
                        }
                    }
                    ps.spawn(K::SmallFlame, &o, at, none);
                }
            }
            A::EnchantingTable => {
                for dz in -2..=2i32 {
                    for dy in 0..=1 {
                        for dx in -2..=2i32 {
                            if dx.abs() != 2 && dz.abs() != 2 {
                                continue;
                            }
                            if self.anim_rng.int(16) != 0 {
                                continue;
                            }
                            let shelf = c.state([p[0] + dx, p[1] + dy, p[2] + dz]);
                            let between = c.state([p[0] + dx / 2, p[1] + dy, p[2] + dz / 2]);
                            if !self.tag(c, shelf, T_ENCH_PROVIDER) || !self.tag(c, between, T_ENCH_TRANSMITTER) {
                                continue;
                            }
                            let r = &mut self.anim_rng;
                            let v = [
                                f64::from(dx) + f64::from(r.f()) - 0.5,
                                f64::from(dy) - f64::from(r.f()) - 1.0,
                                f64::from(dz) + f64::from(r.f()) - 0.5,
                            ];
                            ps.spawn(K::Enchant, &o, [b[0] + 0.5, b[1] + 2.0, b[2] + 0.5], v);
                        }
                    }
                }
            }
            A::EnderChest => {
                for _ in 0..3 {
                    let r = &mut self.anim_rng;
                    let fx = f64::from(r.int(2) * 2 - 1);
                    let fz = f64::from(r.int(2) * 2 - 1);
                    let at = [b[0] + 0.5 + 0.25 * fx, b[1] + f64::from(r.f()), b[2] + 0.5 + 0.25 * fz];
                    let v = [f64::from(r.f()) * fx, (f64::from(r.f()) - 0.5) * 0.125, f64::from(r.f()) * fz];
                    ps.spawn(K::Portal, &o, at, v);
                }
            }
            A::EndGateway => {
                // One particle per visible face (neighbour that does not hide it).
                let count = (0..6).filter(|&d| !c.flag(c.state(add(p, d)), flags::SOLID_RENDER)).count();
                for _ in 0..count {
                    let r = &mut self.anim_rng;
                    let mut at = [b[0] + r.d(), b[1] + r.d(), b[2] + r.d()];
                    let mut v = [(r.d() - 0.5) * 0.5, (r.d() - 0.5) * 0.5, (r.d() - 0.5) * 0.5];
                    let flip = f64::from(r.int(2) * 2 - 1);
                    if r.bool() {
                        at[2] = b[2] + 0.5 + 0.25 * flip;
                        v[2] = f64::from(r.f() * 2.0) * flip;
                    } else {
                        at[0] = b[0] + 0.5 + 0.25 * flip;
                        v[0] = f64::from(r.f() * 2.0) * flip;
                    }
                    ps.spawn(K::Portal, &o, at, v);
                }
            }
            A::EndPortal => {
                let r = &mut self.anim_rng;
                let at = [b[0] + r.d(), b[1] + 0.8, b[2] + r.d()];
                ps.spawn(K::Smoke, &o, at, none);
            }
            A::EndRod => {
                let d = DIRS[facing()];
                let r = &mut self.anim_rng;
                let x = b[0] + 0.55 - f64::from(r.f() * 0.1);
                let y = b[1] + 0.55 - f64::from(r.f() * 0.1);
                let z = b[2] + 0.55 - f64::from(r.f() * 0.1);
                let k = f64::from(0.4 - (r.f() + r.f()) * 0.4);
                if r.int(5) == 0 {
                    let at = [x + f64::from(d[0]) * k, y + f64::from(d[1]) * k, z + f64::from(d[2]) * k];
                    let v = [r.gauss() * 0.005, r.gauss() * 0.005, r.gauss() * 0.005];
                    ps.spawn(K::EndRod, &o, at, v);
                }
            }
            A::NetherPortal => {
                let r = &mut self.anim_rng;
                if r.int(100) == 0 {
                    let pitch = r.f() * 0.4 + 0.8;
                    self.sound_at("minecraft:block.portal.ambient", p, Category::Block, 0.5, pitch);
                }
                let along_x = c.block(c.state(add(p, 4))) == self.nether_portal
                    || c.block(c.state(add(p, 5))) == self.nether_portal;
                for _ in 0..4 {
                    let r = &mut self.anim_rng;
                    let mut at = [b[0] + r.d(), b[1] + r.d(), b[2] + r.d()];
                    let mut v =
                        [f64::from(r.f() - 0.5) * 0.5, f64::from(r.f() - 0.5) * 0.5, f64::from(r.f() - 0.5) * 0.5];
                    let flip = f64::from(r.int(2) * 2 - 1);
                    if along_x {
                        at[2] = b[2] + 0.5 + 0.25 * flip;
                        v[2] = f64::from(r.f() * 2.0) * flip;
                    } else {
                        at[0] = b[0] + 0.5 + 0.25 * flip;
                        v[0] = f64::from(r.f() * 2.0) * flip;
                    }
                    ps.spawn(K::Portal, &o, at, v);
                }
            }
            A::Falling | A::Sand | A::Brushable => {
                if self.anim_rng.int(16) == 0 && self.free(c, c.state(add(p, DOWN))) {
                    let r = &mut self.anim_rng;
                    let at = [b[0] + r.d(), b[1] - 0.05, b[2] + r.d()];
                    ps.spawn(K::FallingDust, &Options { state: s, ..o }, at, none);
                }
                if a == A::Sand {
                    self.sand_sound(c, p);
                }
            }
            A::Leaves { fall, tint, sound } => {
                let below = add(p, DOWN);
                let bs = c.state(below);
                // Raindrops.
                if (c.raining_at)(add(p, UP))
                    && self.anim_rng.int(15) == 1
                    && (!c.flag(bs, flags::CAN_OCCLUDE) || !c.sturdy(bs, UP))
                {
                    let r = &mut self.anim_rng;
                    let at = [b[0] + r.d(), b[1] - 0.05, b[2] + r.d()];
                    ps.spawn(K::DrippingWater, &o, at, none);
                }
                if sound {
                    self.leaves_sound(c, p, s);
                }
                if let Some((kind, chance)) = fall
                    && self.anim_rng.f() < chance
                    && !top_face_full(c.collision(bs))
                {
                    let argb = match tint {
                        Tint::Biome => (c.foliage)(p),
                        Tint::Fixed(v) => v,
                        Tint::None => 0xffff_ffff,
                    };
                    let r = &mut self.anim_rng;
                    let at = [b[0] + r.d(), b[1] - 0.05, b[2] + r.d()];
                    ps.spawn(kind, &Options { argb, ..o }, at, none);
                }
            }
            A::WetSponge => {
                let r = &mut self.anim_rng;
                let d = r.int(6) as usize;
                if d == UP {
                    return;
                }
                let n = c.state(add(p, d));
                if c.flag(s, flags::CAN_OCCLUDE) && c.sturdy(n, opposite(d)) {
                    return;
                }
                let r = &mut self.anim_rng;
                let mut at = b;
                if d == DOWN {
                    at[1] -= 0.05;
                    at[0] += r.d();
                    at[2] += r.d();
                } else {
                    at[1] += r.d() * 0.8;
                    if DIRS[d][0] != 0 {
                        at[2] += r.d();
                        at[0] += if DIRS[d][0] > 0 { 1.1 } else { 0.05 };
                    } else {
                        at[0] += r.d();
                        at[2] += if DIRS[d][2] > 0 { 1.1 } else { 0.05 };
                    }
                }
                ps.spawn(K::DrippingWater, &o, at, none);
            }
            A::Beehive => {
                if c.int_prop(s, "honey_level") < 5 {
                    return;
                }
                for _ in 0..self.anim_rng.int(1) + 1 {
                    self.honey_drip(c, ps, p, s);
                }
            }
            A::CryingObsidian => {
                let r = &mut self.anim_rng;
                if r.int(5) != 0 {
                    return;
                }
                let d = r.int(6) as usize;
                if d == UP {
                    return;
                }
                let n = c.state(add(p, d));
                if c.flag(s, flags::CAN_OCCLUDE) && c.sturdy(n, opposite(d)) {
                    return;
                }
                let r = &mut self.anim_rng;
                let mut off = [0.0; 3];
                for (i, v) in off.iter_mut().enumerate() {
                    *v = if DIRS[d][i] == 0 { r.d() } else { 0.5 + f64::from(DIRS[d][i]) * 0.6 };
                }
                ps.spawn(K::DrippingObsidianTear, &o, [b[0] + off[0], b[1] + off[1], b[2] + off[2]], none);
            }
            A::RespawnAnchor => {
                if c.int_prop(s, "charges") == 0 {
                    return;
                }
                if self.anim_rng.int(100) == 0 {
                    self.sound_at("minecraft:block.respawn_anchor.ambient", p, Category::Block, 1.0, 1.0);
                }
                let r = &mut self.anim_rng;
                let at = [b[0] + 0.5 + (0.5 - r.d()), b[1] + 1.0, b[2] + 0.5 + (0.5 - r.d())];
                let v = [0.0, f64::from(r.f()) * 0.04, 0.0];
                ps.spawn(K::ReversePortal, &o, at, v);
            }
            A::Mycelium => {
                let r = &mut self.anim_rng;
                if r.int(10) == 0 {
                    let at = [b[0] + r.d(), b[1] + 1.1, b[2] + r.d()];
                    ps.spawn(K::Mycelium, &o, at, none);
                }
            }
            A::SporeBlossom => {
                let r = &mut self.anim_rng;
                let at = [b[0] + r.d(), b[1] + 0.7, b[2] + r.d()];
                ps.spawn(K::FallingSporeBlossom, &o, at, none);
                for _ in 0..14 {
                    let r = &mut self.anim_rng;
                    let q = [p[0] + r.between(-10, 10), p[1] - r.int(10), p[2] + r.between(-10, 10)];
                    if c.flag(c.state(q), flags::FULL_COLLISION) {
                        continue;
                    }
                    let r = &mut self.anim_rng;
                    let at = [f64::from(q[0]) + r.d(), f64::from(q[1]) + r.d(), f64::from(q[2]) + r.d()];
                    ps.spawn(K::SporeBlossomAir, &o, at, none);
                }
            }
            A::BubbleColumn => {
                let r = &mut self.anim_rng;
                let (name, v, pitch);
                if c.is(s, "drag", "true") {
                    ps.spawn_visible(K::CurrentDown, &o, [b[0] + 0.5, b[1] + 0.8, b[2]], none);
                    if r.int(200) != 0 {
                        return;
                    }
                    name = "minecraft:block.bubble_column.whirlpool_ambient";
                } else {
                    let up = [0.0, 0.04, 0.0];
                    ps.spawn_visible(K::BubbleColumnUp, &o, [b[0] + 0.5, b[1], b[2] + 0.5], up);
                    let at = [b[0] + f64::from(r.f()), b[1] + f64::from(r.f()), b[2] + f64::from(r.f())];
                    ps.spawn_visible(K::BubbleColumnUp, &o, at, up);
                    if r.int(200) != 0 {
                        return;
                    }
                    name = "minecraft:block.bubble_column.upwards_ambient";
                }
                (v, pitch) = (0.2 + r.f() * 0.2, 0.9 + r.f() * 0.15);
                self.sound(name, b, Category::Block, v, pitch);
            }
            A::WitherRose => {
                // Centre of the shape (centred flower, offset like its model).
                let off = crate::crack::model_offset(c.data, s, p);
                let (x, z) = (b[0] + 0.5 + f64::from(off.x), b[2] + 0.5 + f64::from(off.z));
                for _ in 0..3 {
                    let r = &mut self.anim_rng;
                    if r.bool() {
                        let at = [x + r.d() / 5.0, b[1] + (0.5 - r.d()), z + r.d() / 5.0];
                        ps.spawn(K::Smoke, &o, at, none);
                    }
                }
            }
            A::LightningRod => {
                if !c.thundering
                    || i64::from(self.world.int(200)) > c.game_time.rem_euclid(200)
                    || p[1] != c.surface(p[0], p[2]) - 1
                {
                    return;
                }
                let axis = (0..3).find(|&i| DIRS[facing()][i] != 0).unwrap_or(1);
                let r = &mut self.world;
                for _ in 0..1 + r.int(2) {
                    let mut at = [0.0; 3];
                    let mut v = [0.0; 3];
                    for i in 0..3 {
                        at[i] = b[i] + 0.5 + r.range(-1.0, 1.0) * if i == axis { 0.5 } else { 0.125 };
                    }
                    for (i, vi) in v.iter_mut().enumerate() {
                        *vi = if i == axis { r.range(-1.0, 1.0) } else { 0.0 };
                    }
                    ps.spawn(K::ElectricSpark, &o, at, v);
                }
            }
            A::FireflyBush => {
                let tags = &self.tags;
                let no_leaves = |st: u32| {
                    tags.get(usize::from(c.block(st))).is_some_and(|t| t & T_NO_LEAVES_HEIGHT != 0)
                        || c.fluid(st) != Fluid::None
                };
                if self.anim_rng.int(30) == 0 && c.night && c.height(p[0], p[2], &no_leaves) <= p[1] {
                    self.sound_at("minecraft:block.firefly_bush.idle", p, Category::Ambient, 1.0, 1.0);
                }
                if (c.brightness)(p) <= 13 && self.anim_rng.d() <= 0.7 {
                    let r = &mut self.anim_rng;
                    let at = [b[0] + r.d() * 10.0 - 5.0, b[1] + r.d() * 5.0, b[2] + r.d() * 10.0 - 5.0];
                    ps.spawn(K::Firefly, &o, at, none);
                }
            }
            A::PointedDripstone => {
                if !(c.is(s, "vertical_direction", "down")
                    && c.is(s, "thickness", "tip")
                    && !c.is(s, "waterlogged", "true"))
                {
                    return;
                }
                let v = self.anim_rng.f();
                if v > 0.12 {
                    return;
                }
                if let Some((kind, can_fill)) = self.stalactite_drip(c, p, s)
                    && (v < 0.02 || can_fill)
                {
                    let off = crate::crack::model_offset(c.data, s, p);
                    let at = [b[0] + 0.5 + f64::from(off.x), b[1] + 0.25, b[2] + 0.5 + f64::from(off.z)];
                    ps.spawn(kind, &o, at, none);
                }
            }
            A::RedstoneWire => {
                let power = c.int_prop(s, "power");
                if power == 0 {
                    return;
                }
                let color = wire_color(power);
                for h in HORIZONTAL {
                    let key = ["", "", "north", "south", "west", "east"][h];
                    match c.prop(s, key) {
                        Some("up") => {
                            self.wire_line(ps, p, color, h, UP, -0.5, 0.5);
                            self.wire_line(ps, p, color, DOWN, h, 0.0, 0.5);
                        }
                        Some("side") => self.wire_line(ps, p, color, DOWN, h, 0.0, 0.5),
                        _ => self.wire_line(ps, p, color, DOWN, h, 0.0, 0.3),
                    }
                }
            }
            A::Repeater => {
                if !c.is(s, "powered", "true") {
                    return;
                }
                let d = DIRS[facing()];
                let r = &mut self.anim_rng;
                let x = b[0] + 0.5 + (r.d() - 0.5) * 0.2;
                let y = b[1] + 0.4 + (r.d() - 0.5) * 0.2;
                let z = b[2] + 0.5 + (r.d() - 0.5) * 0.2;
                let mut off = -5.0f32;
                if r.bool() {
                    off = (c.int_prop(s, "delay") * 2 - 1) as f32;
                }
                off /= 16.0;
                let at = [x + f64::from(off * d[0] as f32), y, z + f64::from(off * d[2] as f32)];
                ps.spawn(K::Dust, &Options::dust(REDSTONE, 1.0), at, none);
            }
            A::RedstoneOre => {
                if !lit() {
                    return;
                }
                for (d, step) in DIRS.iter().enumerate() {
                    if c.flag(c.state(add(p, d)), flags::SOLID_RENDER) {
                        continue;
                    }
                    let r = &mut self.world;
                    let mut off = [0.0; 3];
                    for (v, &s) in off.iter_mut().zip(step) {
                        *v = if s != 0 { 0.5 + 0.5625 * f64::from(s) } else { f64::from(r.f()) };
                    }
                    let at = [b[0] + off[0], b[1] + off[1], b[2] + off[2]];
                    ps.spawn(K::Dust, &Options::dust(REDSTONE, 1.0), at, none);
                }
            }
            A::Lever => {
                if !c.is(s, "powered", "true") || self.anim_rng.f() >= 0.25 {
                    return;
                }
                let f = facing();
                let connected = match c.prop(s, "face") {
                    Some("ceiling") => DOWN,
                    Some("floor") => UP,
                    _ => f,
                };
                let (d1, d2) = (DIRS[opposite(f)], DIRS[opposite(connected)]);
                let mut at = [0.0; 3];
                for i in 0..3 {
                    at[i] = b[i] + 0.5 + 0.1 * f64::from(d1[i]) + 0.2 * f64::from(d2[i]);
                }
                ps.spawn(K::Dust, &Options::dust(REDSTONE, 0.5), at, none);
            }
            A::SculkSensor => {
                if !c.is(s, "sculk_sensor_phase", "active") {
                    return;
                }
                let r = &mut self.anim_rng;
                let d = r.int(6) as usize;
                if d == UP || d == DOWN {
                    return;
                }
                let x = b[0] + 0.5 + if DIRS[d][0] == 0 { 0.5 - r.d() } else { f64::from(DIRS[d][0]) * 0.6 };
                let z = b[2] + 0.5 + if DIRS[d][2] == 0 { 0.5 - r.d() } else { f64::from(DIRS[d][2]) * 0.6 };
                let v = [0.0, f64::from(r.f()) * 0.04, 0.0];
                let t = Options { rgb2: Options::dust(REDSTONE, 1.0).rgb, ..Options::dust(SCULK, 1.0) };
                ps.spawn(K::DustColorTransition, &t, [x, b[1] + 0.25, z], v);
            }
            A::DriedGhast => {
                let at = b.map(|v| v + 0.5);
                if !c.is(s, "waterlogged", "true") {
                    if self.anim_rng.int(40) == 0 && self.tag(c, c.state(add(p, DOWN)), T_DRIED_GHAST) {
                        self.sound("minecraft:block.dried_ghast.ambient", at, Category::Block, 1.0, 1.0);
                    }
                    if self.anim_rng.int(6) == 0 {
                        ps.spawn(K::WhiteSmoke, &o, at, [0.0, 0.02, 0.0]);
                    }
                } else {
                    if self.anim_rng.int(40) == 0 {
                        self.sound("minecraft:block.dried_ghast.ambient_water", at, Category::Block, 1.0, 1.0);
                    }
                    let r = &mut self.anim_rng;
                    if r.int(6) == 0 {
                        let x = at[0] + f64::from((r.f() * 2.0 - 1.0) / 3.0);
                        let y = at[1] + 0.4;
                        let z = at[2] + f64::from((r.f() * 2.0 - 1.0) / 3.0);
                        let v = [0.0, f64::from(r.f()), 0.0];
                        ps.spawn(K::HappyVillager, &o, [x, y, z], v);
                    }
                }
            }
            A::DeadBush => {
                if self.anim_rng.int(130) != 0 {
                    return;
                }
                let below = c.state(add(p, DOWN));
                if (c.block(below) == self.red_sand || self.tag(c, below, T_TERRACOTTA)) && self.anim_rng.int(3) != 0 {
                    return;
                }
                if self.desert_veg(c, add(p, DOWN)) {
                    self.sound("minecraft:block.deadbush.idle", b, Category::Ambient, 1.0, 1.0);
                }
            }
            A::DryGrass => {
                if self.anim_rng.int(200) == 0 && self.desert_veg(c, add(p, DOWN)) {
                    // Player-bound sound (follows their feet).
                    self.sound("minecraft:block.dry_grass.ambient", c.player_pos, Category::Ambient, 1.0, 1.0);
                }
            }
            A::Eyeblossom => {
                if self.anim_rng.int(700) == 0 && c.block(c.state(add(p, DOWN))) == self.pale_moss {
                    self.sound("minecraft:block.eyeblossom.idle", b, Category::Ambient, 1.0, 1.0);
                }
            }
            A::HangingMoss => {
                if self.anim_rng.int(500) == 0 {
                    let above = c.state(add(p, UP));
                    if self.tag(c, above, T_PALE_OAK_LOGS) || c.block(above) == self.pale_oak_leaves {
                        self.sound("minecraft:block.pale_hanging_moss.idle", b, Category::Ambient, 1.0, 1.0);
                    }
                }
            }
            A::CreakingHeart => {
                if c.night
                    && !c.is(s, "creaking_heart_state", "uprooted")
                    && self.anim_rng.int(16) == 0
                    && (0..6).all(|d| self.tag(c, c.state(add(p, d)), T_PALE_OAK_LOGS))
                {
                    self.sound("minecraft:block.creaking_heart.idle", b, Category::Block, 1.0, 1.0);
                }
            }
            A::PotentSulfur => {
                if c.is(s, "potent_sulfur_state", "dry") {
                    return;
                }
                let above = c.state(add(p, UP));
                let water_source = c.fluid(above) == Fluid::Water
                    && c.w.phys.fluids.source.get(above as usize).copied().unwrap_or(false);
                if !water_source {
                    return;
                }
                for _ in 0..2 {
                    let r = &mut self.anim_rng;
                    let at = [b[0] + f64::from(r.f()), b[1] + 1.0 + f64::from(r.f()), b[2] + f64::from(r.f())];
                    ps.spawn_visible(K::SulfurBubbles, &o, at, none);
                }
                if self.anim_rng.int(10) == 0 {
                    self.sound("minecraft:block.potent_sulfur.noxious_gas", b, Category::Ambient, 1.0, 1.0);
                }
            }
        }
    }

    /// Fire: sound, smoke on the flammable sides if it rests on nothing, otherwise above.
    fn fire(&mut self, c: &Ctx<'_>, ps: &mut Particles, p: [i32; 3], soul: bool) {
        let b = f3(p);
        let r = &mut self.anim_rng;
        if r.int(24) == 0 {
            let (v, pitch) = (1.0 + r.f(), r.f() * 0.7 + 0.3);
            self.sound_at("minecraft:block.fire.ambient", p, Category::Block, v, pitch);
        }
        let burns = |st: u32| soul || (self.tag(c, st, T_FLAMMABLE) && !c.is(st, "waterlogged", "true"));
        let below = c.state(add(p, DOWN));
        let o = Options::default();
        if !burns(below) && !c.sturdy(below, UP) {
            // West, east, north, south, up: (fixed axis, side).
            let sides: [(usize, usize, bool); 5] =
                [(4, 0, false), (5, 0, true), (2, 2, false), (3, 2, true), (1, 1, true)];
            let lit = sides.map(|(d, _, _)| burns(c.state(add(p, d))));
            for ((_, axis, far), lit) in sides.into_iter().zip(lit) {
                if !lit {
                    continue;
                }
                for _ in 0..2 {
                    let r = &mut self.anim_rng;
                    // Three draws in x, y, z order; the one on the fixed axis gives the offset from the edge.
                    let mut at = [0.0f64; 3];
                    for (k, c) in at.iter_mut().enumerate() {
                        let d = r.d();
                        *c = if k == axis {
                            let edge = d * f64::from(0.1f32);
                            b[k] + if far { 1.0 - edge } else { edge }
                        } else {
                            b[k] + d
                        };
                    }
                    ps.spawn(K::LargeSmoke, &o, at, [0.0; 3]);
                }
            }
        } else {
            for _ in 0..3 {
                let r = &mut self.anim_rng;
                let at = [b[0] + r.d(), b[1] + r.d() * 0.5 + 0.5, b[2] + r.d()];
                ps.spawn(K::LargeSmoke, &o, at, [0.0; 3]);
            }
        }
    }

    /// Air, fire, liquid or replaceable.
    fn free(&self, c: &Ctx<'_>, s: u32) -> bool {
        c.air(s) || self.tag(c, s, T_FIRE) || c.flag(s, flags::LIQUID) || c.flag(s, flags::REPLACEABLE)
    }

    /// Honey drip under a full beehive.
    fn honey_drip(&mut self, c: &Ctx<'_>, ps: &mut Particles, p: [i32; 3], s: u32) {
        if c.fluid(s) != Fluid::None || self.world.f() < 0.3 {
            return;
        }
        let shape = c.collision(s);
        let (bottom, top) = min_max(shape, 1);
        if top < 1.0 || self.tag(c, s, T_IMPERMEABLE) {
            return;
        }
        let y = f64::from(p[1]);
        if bottom > 0.0 {
            self.honey_in(ps, p, shape, y + bottom - 0.05);
        } else {
            let below = c.state(add(p, DOWN));
            let (_, below_top) = min_max(c.collision(below), 1);
            if (below_top < 1.0 || !c.flag(below, flags::FULL_COLLISION)) && c.fluid(below) == Fluid::None {
                self.honey_in(ps, p, shape, y - 0.05);
            }
        }
    }

    fn honey_in(&mut self, ps: &mut Particles, p: [i32; 3], shape: &[rc_data::Aabb], y: f64) {
        let (x0, x1) = min_max(shape, 0);
        let (z0, z1) = min_max(shape, 2);
        let (bx, bz) = (f64::from(p[0]), f64::from(p[2]));
        let (a, b) = (self.world.d(), self.world.d());
        let at = [bx + x0 + a * (x1 - x0), y, bz + z0 + b * (z1 - z0)];
        ps.spawn(K::DrippingHoney, &Options::default(), at, [0.0; 3]);
    }

    /// Dust line of a redstone wire (density 0.2 per block of length).
    #[allow(clippy::too_many_arguments)]
    fn wire_line(
        &mut self,
        ps: &mut Particles,
        p: [i32; 3],
        color: u32,
        side: usize,
        along: usize,
        from: f32,
        to: f32,
    ) {
        let span = to - from;
        let r = &mut self.anim_rng;
        if r.f() >= 0.2 * span {
            return;
        }
        let t = from + span * r.f();
        let mut at = [0.0; 3];
        for i in 0..3 {
            at[i] = f64::from(p[i])
                + 0.5
                + f64::from(0.4375f32 * DIRS[side][i] as f32)
                + f64::from(t * DIRS[along][i] as f32);
        }
        ps.spawn(K::Dust, &Options::dust(color, 1.0), at, [0.0; 3]);
    }

    /// Drip at the tip of a stalactite: drip type and "the fluid can fill a cauldron".
    pub fn stalactite_drip(&self, c: &Ctx<'_>, p: [i32; 3], s: u32) -> Option<(K, bool)> {
        stalactite_fluid(c.data, &|q| c.state(q), c.w.phys, c.w.min_y + c.w.height, self.mud, p, s, c.evaporates)
    }

    /// Sand sounds (desert): air block above, 1/2100, three columns at 8 blocks out of four.
    fn sand_sound(&mut self, c: &Ctx<'_>, p: [i32; 3]) {
        if c.block(c.state(add(p, UP))) != self.air_block {
            return;
        }
        if self.anim_rng.int(2100) != 0 || !self.sand_columns(c, p) {
            return;
        }
        self.sound("minecraft:block.sand.idle", f3(p), Category::Ambient, 1.0, 1.0);
    }

    fn sand_columns(&self, c: &Ctx<'_>, p: [i32; 3]) -> bool {
        let mut found = 0;
        for (checked, &h) in HORIZONTAL.iter().enumerate() {
            let q = [p[0] + DIRS[h][0] * 8, p[1], p[2] + DIRS[h][2] * 8];
            if self.column_has_sand(c, q) {
                found += 1;
                if found > 3 {
                    return true;
                }
            }
            let remaining = 4 - (checked as i32 + 1);
            if remaining + found < 3 {
                return false;
            }
        }
        false
    }

    fn column_has_sand(&self, c: &Ctx<'_>, q: [i32; 3]) -> bool {
        let surface = c.surface(q[0], q[2]) - 1;
        if (surface - q[1]).abs() > 5 {
            let mut y = q[1] + 6;
            let mut above = c.state([q[0], y, q[2]]);
            y -= 1;
            for _ in 0..10 {
                let cur = c.state([q[0], y, q[2]]);
                if c.air(above) && self.tag(c, cur, T_DESERT_SAND) {
                    return true;
                }
                above = cur;
                y -= 1;
            }
            false
        } else {
            c.air(c.state([q[0], surface + 1, q[2]])) && self.tag(c, c.state([q[0], surface, q[2]]), T_DESERT_SAND)
        }
    }

    /// Dry desert vegetation: two "desert" blocks one under the other under the plant.
    fn desert_veg(&self, c: &Ctx<'_>, below: [i32; 3]) -> bool {
        self.tag(c, c.state(below), T_DESERT_VEG) && self.tag(c, c.state(add(below, DOWN)), T_DESERT_VEG)
    }

    /// Poplar leaf sound: 1/300, with a neighbouring trunk and three identical leaves.
    fn leaves_sound(&mut self, c: &Ctx<'_>, p: [i32; 3], s: u32) {
        if self.anim_rng.int(300) != 0 {
            return;
        }
        let me = c.block(s);
        let (mut logs, mut leaves) = (0, 0);
        for h in HORIZONTAL {
            let n = c.state(add(p, h));
            if self.tag(c, n, T_POPLAR_LOGS) {
                logs += 1;
            }
            if c.block(n) == me {
                leaves += 1;
            }
            if logs == 1 && leaves == 3 {
                self.sound("minecraft:block.poplar_leaves.ambient", f3(p), Category::Ambient, 1.0, 1.0);
                return;
            }
        }
    }
}

impl crate::App {
    /// Block animation for one tick (world not frozen, player placed), after the world tick
    /// and before the particle one; sounds played immediately.
    pub(crate) fn tick_block_anim(&mut self) {
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let w = TickWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            player: None,
            scope: None,
        };
        let weather_ok = self.env.has_sky && !self.dimension.ends_with("the_end");
        let rain = weather_ok && self.env.env.weather.rain > 0.2;
        let thundering = weather_ok && self.env.env.weather.thunder() > 0.9;
        let night = self.env.env.eval_global(rc_env::Attr::FireflyBushSounds)[0] > 0.5;
        let darken = (15.0 - self.env.env.eval_global(rc_env::Attr::SkyLightLevel)[0]) as i32;
        let has_sky = self.env.has_sky;
        // Marker: creative, barrier or light block in the main hand.
        let held = self.inventory.hotbar(self.hud.selected).map(|s| s.item);
        let marker = if self.env.game_mode == 1 {
            self.block_anim.marker_items.iter().find(|m| Some(m.0) == held).map(|m| m.1)
        } else {
            None
        };
        let (weather, env, palette) = (&self.weather, &self.env, &self.palette);
        let raining_at = |p: [i32; 3]| {
            rain && view.light_pair(p[0], p[1], p[2]).1 == 15
                && weather.heights.get(p[0], p[2], weather.climate.sea_level) <= p[1]
                && weather.climate.at(&view, p[0], p[1], p[2]) == crate::weather::Precip::Rain
        };
        let center = |p: [i32; 3]| [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let biome = |p: [i32; 3]| env.biome_at(&view, center(p));
        let foliage = |p: [i32; 3]| palette.get(env.biome_at(&view, center(p)) as usize).map_or(0x48b518, |c| c[1]);
        let brightness = |p: [i32; 3]| view.raw_brightness(p[0], p[1], p[2], has_sky, darken);
        let ctx = Ctx {
            w: &w,
            data: &self.data,
            player: self.player.pos.map(|v| v.floor() as i32),
            player_pos: self.player.pos,
            marker,
            raining_at: &raining_at,
            thundering,
            game_time: self.env.env.clocks.game_time,
            night,
            brightness: &brightness,
            biome: &biome,
            foliage: &foliage,
            evaporates: self.dimension.ends_with("the_nether"),
        };
        self.block_anim.tick(&ctx, &mut self.particles);
        let sounds = self.block_anim.take_sounds();
        if let Some(a) = self.audio.get() {
            for s in sounds {
                a.play(s.name, s.category, Some(s.pos), s.volume, s.pitch, None);
            }
        }
    }
}

/// Fluid above a stalactite (tip down, at `p`): root = first block above
/// that is no longer the column (10 steps, stalactite path of the same direction), fluid read at the block
/// ABOVE the root; wet mud = water outside a dimension where water evaporates; with no fluid,
/// the dimension's default drip (lava if water evaporates). Returns the drip type and "water
/// or lava" (the only fluids that fill a cauldron).
#[allow(clippy::too_many_arguments)]
pub fn stalactite_fluid(
    data: &VersionData,
    state: &dyn Fn([i32; 3]) -> u32,
    phys: &rc_physics::Tables,
    top: i32,
    mud: u16,
    p: [i32; 3],
    s: u32,
    evaporates: bool,
) -> Option<(K, bool)> {
    let dripstone = data.state(s)?.block;
    let down = |st: u32| data.state_properties(st).any(|(k, v)| k == "vertical_direction" && v == "down");
    if !down(s) {
        return None;
    }
    let mut root = None;
    for i in 1..11 {
        let q = [p[0], p[1] + i, p[2]];
        let st = state(q);
        if data.state(st).map(|x| x.block) != Some(dripstone) {
            root = Some(q);
            break;
        }
        if q[1] >= top || !down(st) {
            return None;
        }
    }
    let r = root?;
    let above = [r[0], r[1] + 1, r[2]];
    let a = state(above);
    let fluid =
        if data.state(a).map(|x| x.block) == Some(mud) && !evaporates { Fluid::Water } else { phys.fluids.fluid(a) };
    Some(match fluid {
        Fluid::Lava => (K::DrippingDripstoneLava, true),
        Fluid::Water => (K::DrippingDripstoneWater, true),
        Fluid::None if evaporates => (K::DrippingDripstoneLava, false),
        Fluid::None => (K::DrippingDripstoneWater, false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangular_offsets_stay_in_radius() {
        let mut r = Rand::new(3);
        let mut hist = [0u32; 63];
        for _ in 0..200_000 {
            let d = r.int(32) - r.int(32);
            assert!((-31..=31).contains(&d));
            hist[(d + 31) as usize] += 1;
        }
        // Triangular distribution: the centre is ~32 times more likely than the edges.
        assert!(hist[31] > 20 * hist[0].max(1));
    }

    #[test]
    fn wire_colors_match_formula() {
        assert_eq!(wire_color(0), 0x4c_0000);
        // 0.7 − 0.5 in single precision is 0.19999999: 50 and not 51 (like the game).
        assert_eq!(wire_color(15), 0xff_3200);
    }

    #[test]
    fn classify_known_blocks() {
        assert_eq!(classify("minecraft:torch"), A::Torch(K::Flame));
        assert_eq!(classify("minecraft:soul_wall_torch"), A::WallTorch(K::SoulFireFlame));
        assert_eq!(classify("minecraft:lime_candle"), A::Candle);
        assert_eq!(classify("minecraft:lime_candle_cake"), A::CandleCake);
        assert_eq!(classify("minecraft:waxed_oxidized_lightning_rod"), A::LightningRod);
        assert_eq!(classify("minecraft:red_concrete_powder"), A::Falling);
        assert_eq!(classify("minecraft:stone"), A::None);
    }

    /// State of a block whose given properties have these values.
    fn state_of(d: &VersionData, name: &str, props: &[(&str, &str)]) -> u32 {
        let b = d.blocks.iter().find(|b| b.name == name).expect(name);
        (b.first_state..b.first_state + b.state_count)
            .find(|&s| props.iter().all(|(k, v)| d.state_properties(s).any(|p| p.0 == *k && p.1 == *v)))
            .expect("state")
    }

    #[test]
    fn stalactite_reads_fluid_above_root() {
        let Ok(d) = rc_data::load_or_prepare("26.3", |_| {}) else {
            eprintln!("26.3 data missing: test skipped");
            return;
        };
        eprintln!("26.3 data: {} blocks", d.blocks.len());
        let phys = rc_physics::Tables::new(&d);
        let tip = state_of(&d, "minecraft:pointed_dripstone", &[("vertical_direction", "down"), ("thickness", "tip")]);
        let base =
            state_of(&d, "minecraft:pointed_dripstone", &[("vertical_direction", "down"), ("thickness", "frustum")]);
        let stone = state_of(&d, "minecraft:stone", &[]);
        let water = state_of(&d, "minecraft:water", &[("level", "0")]);
        let lava = state_of(&d, "minecraft:lava", &[("level", "0")]);
        let mud = d.blocks.iter().position(|b| b.name == "minecraft:mud").unwrap() as u16;
        let mud_state = state_of(&d, "minecraft:mud", &[]);
        // Column: tip (y 0), trunk (1), stone (2); above the stone (3): `top`.
        let column = |top: u32| {
            move |q: [i32; 3]| match q[1] {
                0 => tip,
                1 => base,
                2 => stone,
                3 => top,
                _ => 0,
            }
        };
        let get = |top: u32, nether: bool| stalactite_fluid(&d, &column(top), &phys, 320, mud, [0, 0, 0], tip, nether);
        // The fluid counts above the root (the stone), not in the root.
        assert_eq!(get(water, false), Some((K::DrippingDripstoneWater, true)));
        assert_eq!(get(lava, false), Some((K::DrippingDripstoneLava, true)));
        assert_eq!(get(0, false), Some((K::DrippingDripstoneWater, false)));
        assert_eq!(get(0, true), Some((K::DrippingDripstoneLava, false)));
        // Mud: water, except where water evaporates.
        assert_eq!(get(mud_state, false), Some((K::DrippingDripstoneWater, true)));
        assert_eq!(get(mud_state, true), Some((K::DrippingDripstoneLava, false)));
        // Tip pointing up: nothing.
        let up = state_of(&d, "minecraft:pointed_dripstone", &[("vertical_direction", "up"), ("thickness", "tip")]);
        assert_eq!(stalactite_fluid(&d, &column(water), &phys, 320, mud, [0, 0, 0], up, false), None);
    }

    #[test]
    fn top_face() {
        let full = rc_data::Aabb { min: [0.0; 3], max: [1.0; 3] };
        let slab = rc_data::Aabb { min: [0.0; 3], max: [1.0, 0.5, 1.0] };
        assert!(top_face_full(&[full]));
        assert!(!top_face_full(&[slab]));
        assert!(!top_face_full(&[]));
    }
}
