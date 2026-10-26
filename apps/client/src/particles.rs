//! Particle engine (`docs/specs/particles.md`): quad queue (16,384 FIFO, progressive
//! admission), waiting queue emptied at the end of the tick, emitters without rendering, common physics
//! (collisions against the block collision shapes), sprites of the `particles/*.json`
//! definitions packed in an atlas, block fragments (block texture array) and
//! item fragments (item sprites copied into the atlas).
//!
//! Instanced rendering: one instance per particle, camera-facing corners computed in the shader. Each
//! particle has its own generator: the tick is spread over the pool above a threshold.

use hashbrown::HashMap;
use std::cell::RefCell;
use std::time::Instant;

use rayon::prelude::*;
use rc_assets::texture::Image;
use rc_physics::{Aabb, Fluid};
use rc_world::ChunkGrid;
use serde_json::Value;

/// Capacity of a queue (§ 2.1) and threshold of the progressive admission.
const CAPACITY: usize = 16_384;
const CROWDED: usize = 12_288;
/// Distance² beyond which a non-forced particle is not created (§ 1.2).
const MAX_DIST2: f64 = 1024.0;
/// Particles from which the tick moves to the pool (bench: 1000 → 0.12 ms sequential
/// against 0.17 ms on the pool; 10000 → 1.6 ms against 0.78 ms).
const PARALLEL_MIN: usize = 2048;

// ---------- Generator ----------

/// SplitMix64: 8 bytes per particle, independent draws.
#[derive(Clone, Copy, Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform [0, 1) as a double.
    fn d(&mut self) -> f64 {
        (self.next() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform [0, 1) as a single.
    fn f(&mut self) -> f32 {
        (self.next() >> 40) as f32 * (1.0 / (1u32 << 24) as f32)
    }

    fn int(&mut self, n: u32) -> i32 {
        (((self.next() >> 32) * u64::from(n)) >> 32) as i32
    }

    fn gauss(&mut self) -> f64 {
        let u = self.d().max(1e-300);
        let v = self.d();
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }

    fn fork(&mut self) -> Rng {
        Rng(self.next())
    }
}

// ---------- Types ----------

macro_rules! kinds {
    ($($v:ident = $n:literal),* $(,)?) => {
        /// Particle types supported (names of the `minecraft:particle_type` registry).
        #[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
        pub enum Kind { $($v),* }
        const KINDS: &[(Kind, &str)] = &[$((Kind::$v, $n)),*];
    };
}

kinds! {
    Block = "block", BlockCrumble = "block_crumble", DustPillar = "dust_pillar", Item = "item",
    ItemSnowball = "item_snowball", ItemSlime = "item_slime", ItemCobweb = "item_cobweb", Poof = "poof",
    Smoke = "smoke", LargeSmoke = "large_smoke", Ash = "ash", WhiteAsh = "white_ash", Flame = "flame",
    SoulFireFlame = "soul_fire_flame", SmallFlame = "small_flame", CopperFireFlame = "copper_fire_flame",
    Lava = "lava", DrippingWater = "dripping_water", DrippingLava = "dripping_lava",
    FallingWater = "falling_water", FallingLava = "falling_lava", LandingLava = "landing_lava",
    Rain = "rain", Splash = "splash", Bubble = "bubble", BubblePop = "bubble_pop", Crit = "crit",
    EnchantedHit = "enchanted_hit", DamageIndicator = "damage_indicator", Heart = "heart",
    AngryVillager = "angry_villager", HappyVillager = "happy_villager", Composter = "composter",
    EggCrack = "egg_crack", Mycelium = "mycelium", Dolphin = "dolphin", Note = "note", Portal = "portal",
    Enchant = "enchant", Nautilus = "nautilus", Explosion = "explosion",
    ExplosionEmitter = "explosion_emitter", Cloud = "cloud", Sneeze = "sneeze", Dust = "dust",
    EntityEffect = "entity_effect", Effect = "effect", InstantEffect = "instant_effect", Witch = "witch",
    CampfireCosySmoke = "campfire_cosy_smoke", CampfireSignalSmoke = "campfire_signal_smoke",
    FallingDust = "falling_dust", Snowflake = "snowflake", Glow = "glow", ElectricSpark = "electric_spark",
    WaxOn = "wax_on", WaxOff = "wax_off", Scrape = "scrape", TotemOfUndying = "totem_of_undying",
    EndRod = "end_rod", SweepAttack = "sweep_attack", Flash = "flash",
    WhiteSmoke = "white_smoke", DragonBreath = "dragon_breath", SculkCharge = "sculk_charge",
    SculkChargePop = "sculk_charge_pop", Shriek = "shriek", TrialDetection = "trial_spawner_detection",
    TrialDetectionOminous = "trial_spawner_detection_ominous", VaultConnection = "vault_connection",
    TrialOmen = "trial_omen", RaidOmen = "raid_omen", OminousSpawning = "ominous_spawning",
    DrippingDripstoneWater = "dripping_dripstone_water", FallingDripstoneWater = "falling_dripstone_water",
    DrippingDripstoneLava = "dripping_dripstone_lava", FallingDripstoneLava = "falling_dripstone_lava",
    BlockMarker = "block_marker", Underwater = "underwater", SporeBlossomAir = "spore_blossom_air",
    CrimsonSpore = "crimson_spore", WarpedSpore = "warped_spore", FallingSporeBlossom = "falling_spore_blossom",
    FallingNectar = "falling_nectar", DrippingHoney = "dripping_honey", FallingHoney = "falling_honey",
    LandingHoney = "landing_honey", DrippingObsidianTear = "dripping_obsidian_tear",
    FallingObsidianTear = "falling_obsidian_tear", LandingObsidianTear = "landing_obsidian_tear",
    ReversePortal = "reverse_portal", BubbleColumnUp = "bubble_column_up", CurrentDown = "current_down",
    CherryLeaves = "cherry_leaves", PaleOakLeaves = "pale_oak_leaves", RedPoplarLeaves = "red_poplar_leaves",
    OrangePoplarLeaves = "orange_poplar_leaves", YellowPoplarLeaves = "yellow_poplar_leaves",
    TintedLeaves = "tinted_leaves", Firefly = "firefly", DustColorTransition = "dust_color_transition",
    SulfurBubbles = "sulfur_bubbles", Firework = "firework", SonicBoom = "sonic_boom", Gust = "gust",
    SmallGust = "small_gust", GustEmitterLarge = "gust_emitter_large", GustEmitterSmall = "gust_emitter_small",
    SquidInk = "squid_ink", GlowSquidInk = "glow_squid_ink", Soul = "soul", SculkSoul = "sculk_soul", Spit = "spit",
    Fishing = "fishing", DustPlume = "dust_plume", Infested = "infested", PauseMobGrowth = "pause_mob_growth",
    ResetMobGrowth = "reset_mob_growth", NoxiousGas = "noxious_gas", SulfurCubeGoo = "sulfur_cube_goo",
    NoxiousGasCloud = "noxious_gas_cloud", Vibration = "vibration",
}

use Kind as K;

impl Kind {
    pub fn name(self) -> &'static str {
        KINDS[self as usize].1
    }

    /// Registry name, with or without `minecraft:`.
    pub fn from_name(name: &str) -> Option<Kind> {
        let n = name.strip_prefix("minecraft:").unwrap_or(name);
        KINDS.iter().find(|(_, s)| *s == n).map(|&(k, _)| k)
    }

    /// Types forced by nature (§ 1.3, asterisk).
    fn forced(self) -> bool {
        matches!(
            self,
            K::DamageIndicator
                | K::ExplosionEmitter
                | K::Explosion
                | K::Poof
                | K::SweepAttack
                | K::CampfireCosySmoke
                | K::CampfireSignalSmoke
                | K::Glow
                | K::WaxOn
                | K::WaxOff
                | K::ElectricSpark
                | K::Scrape
                | K::SculkCharge
                | K::SculkChargePop
                | K::TrialDetection
                | K::TrialDetectionOminous
                | K::VaultConnection
                | K::OminousSpawning
                | K::Gust
                | K::GustEmitterLarge
                | K::GustEmitterSmall
                | K::SonicBoom
                | K::Spit
                | K::SquidInk
                | K::GlowSquidInk
                | K::BlockMarker
        )
    }

    /// Sprite chosen by age (recomputed every tick).
    fn animated(self) -> bool {
        matches!(
            self,
            K::Poof
                | K::Smoke
                | K::LargeSmoke
                | K::BubblePop
                | K::Explosion
                | K::Cloud
                | K::Sneeze
                | K::Dust
                | K::DustColorTransition
                | K::EntityEffect
                | K::Effect
                | K::InstantEffect
                | K::Witch
                | K::FallingDust
                | K::Snowflake
                | K::Glow
                | K::ElectricSpark
                | K::WaxOn
                | K::WaxOff
                | K::Scrape
                | K::TotemOfUndying
                | K::EndRod
                | K::Firework
                | K::SonicBoom
                | K::Gust
                | K::SmallGust
                | K::SquidInk
                | K::GlowSquidInk
                | K::Soul
                | K::SculkSoul
                | K::Spit
                | K::DustPlume
                | K::Infested
                | K::NoxiousGas
                | K::SweepAttack
                | K::WhiteSmoke
                | K::DragonBreath
                | K::SculkCharge
                | K::SculkChargePop
                | K::TrialDetection
                | K::TrialDetectionOminous
                | K::TrialOmen
                | K::RaidOmen
        )
    }

    /// Size × `clamp((age + t)/life·32, 0, 1)` ("grows").
    fn grows(self) -> bool {
        matches!(
            self,
            K::Smoke
                | K::LargeSmoke
                | K::Ash
                | K::WhiteAsh
                | K::Crit
                | K::EnchantedHit
                | K::DamageIndicator
                | K::Heart
                | K::AngryVillager
                | K::Note
                | K::Cloud
                | K::Sneeze
                | K::Dust
                | K::DustColorTransition
                | K::FallingDust
                | K::DustPlume
                | K::NoxiousGas
                | K::WhiteSmoke
                | K::DragonBreath
                | K::TrialDetection
                | K::TrialDetectionOminous
        )
    }

    fn flame(self) -> bool {
        matches!(self, K::Flame | K::SoulFireFlame | K::SmallFlame | K::CopperFireFlame)
    }

    /// "Spell" template: effects, witch, omens, infestation.
    fn spell(self) -> bool {
        matches!(
            self,
            K::EntityEffect | K::Effect | K::InstantEffect | K::Witch | K::TrialOmen | K::RaidOmen | K::Infested
        )
    }

    /// Fluid of a drip: it disappears on entering it (honey, tears, spores: none).
    fn drip_fluid(self) -> Option<Fluid> {
        match self {
            K::DrippingWater | K::FallingWater | K::DrippingDripstoneWater | K::FallingDripstoneWater => {
                Some(Fluid::Water)
            }
            K::DrippingLava | K::FallingLava | K::LandingLava | K::DrippingDripstoneLava | K::FallingDripstoneLava => {
                Some(Fluid::Lava)
            }
            _ => None,
        }
    }

    /// Cap of live particles per category (§ 2.1): spore blossom spores.
    fn limited(self) -> bool {
        self == K::SporeBlossomAir
    }
}

/// Trail spark to create: position, colour, fade, twinkle flag.
type SparkSeed = ([f64; 3], [f32; 3], Option<[f32; 3]>, u8);

/// Firework spark: trail, twinkle.
const SPARK_TRAIL: u8 = 1;
const SPARK_TWINKLE: u8 = 2;

/// Colour of the tears of crying obsidian.
const TEAR: [f32; 3] = [0.511_718_75, 0.031_25, 0.890_625];

/// Cap of the "spore blossom" category.
const SPORE_LIMIT: usize = 1000;

/// Progress of the life, bounded to [0, 1].
fn progress(age: f32, life: i32) -> f32 {
    (age / life as f32).clamp(0.0, 1.0)
}

/// Fade in then fade out: rise up to `fade_in`, fall during the last
/// `fade_out` of the life (firefly).
fn fade(p: f32, fade_out: f32, fade_in: f32) -> f32 {
    if p >= 1.0 - fade_out {
        (1.0 - p) / fade_out
    } else if p <= fade_in {
        p / fade_in
    } else {
        1.0
    }
}

/// Options of a particle (network codec: `entities.md`), neutral default values.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Block state (`block`, `falling_dust`, `dust_pillar`, `block_crumble`).
    pub state: u32,
    /// Item (`item`), network id.
    pub item: i32,
    /// RGB of `dust`, `effect`, `instant_effect`.
    pub rgb: [f32; 3],
    /// ARGB of `entity_effect`, `flash`.
    pub argb: u32,
    /// Scale of `dust`; generic scale of the block fragments.
    pub scale: f32,
    /// Power (§ 2.2): `effect`, `instant_effect`, block fragments.
    pub power: f32,
    /// Ignores distance and setting.
    pub force: bool,
    /// End colour of `dust_color_transition` (`rgb` = start colour).
    pub rgb2: [f32; 3],
    /// `vibration`: destination and duration of the trip (ticks).
    pub vib: Option<(rc_proto_v777::entity::VibDest, i32)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            state: 0,
            item: -1,
            rgb: [1.0, 0.0, 0.0],
            argb: 0xffff_ffff,
            scale: 1.0,
            power: 1.0,
            force: false,
            rgb2: [1.0, 0.0, 0.0],
            vib: None,
        }
    }
}

impl Options {
    /// Options of a protocol particle (`level_particles` packet, entity metadata).
    pub fn from_net(p: &rc_proto_v777::entity::Particle, force: bool) -> Options {
        let c = p.color;
        Options {
            state: p.state.max(0) as u32,
            item: p.item,
            rgb: rgb_of(c),
            argb: c,
            scale: p.scale,
            power: p.scale,
            force,
            rgb2: rgb_of(p.color2),
            vib: p.vib,
        }
    }

    /// Redstone dust (`dust`, integer colour, scale).
    pub fn dust(rgb: u32, scale: f32) -> Options {
        Options { rgb: rgb_of(rgb), scale, ..Options::default() }
    }
}

// ---------- Resources ----------

/// Block as seen by the particles: "particle" texture (index of the block textures).
#[derive(Clone, Copy)]
struct BlockPart {
    tex: u32,
    /// No fragments: air, moving piston, no texture.
    none: bool,
    /// No rendering (invisible) but not air.
    invisible: bool,
}

/// Sprites, sprite sets and tables per block and per item; the atlas image goes to the GPU
/// at the first preparation.
pub struct ParticleAssets {
    /// Rectangle (x, y, w, h) in pixels of each sprite; 0 = missing sprite.
    rects: Vec<[f32; 4]>,
    translucent: Vec<bool>,
    /// Sprite set per type (order of `particles/<type>.json`).
    sets: Vec<Vec<u16>>,
    blocks: Vec<BlockPart>,
    state_block: Vec<u16>,
    /// "Particle" sprites of the layers of each item (ground rendering, default stack), drawn
    /// at random; empty or 0: missing.
    items: Vec<Vec<u16>>,
    snowball: u16,
    slime: u16,
    cobweb: u16,
    /// Side of the block textures (pixels).
    block_size: f32,
    image: Option<Image>,
    /// Per state: "particle" sprite copied into the particle atlas when the block's model
    /// has no faces (barrier, light: item texture), 0 otherwise (creative markers).
    markers: Vec<u16>,
}

// Resource cache (`rc_assets::cache`).
rc_assets::codec_struct!(BlockPart { tex, none, invisible });
rc_assets::codec_struct!(ParticleAssets {
    rects,
    translucent,
    sets,
    blocks,
    state_block,
    items,
    snowball,
    slime,
    cobweb,
    block_size,
    image,
    markers
});

/// Default stack placed on the ground, without carrier: evaluation of the item models of the shards.
struct GroundItem;

impl rc_assets::item::StackView for GroundItem {
    fn has(&self, _component: &str) -> bool {
        false
    }
}

impl rc_assets::item::EvalEnv for GroundItem {
    fn has_holder(&self) -> bool {
        false
    }

    fn display_context(&self) -> &str {
        "ground"
    }
}

/// Biome colour of kind `kind` (`rc_data::tint`, grass to water) at block `at`, blended like
/// the game: integer average over (2r + 1)² blocks at the same height, each taken from the biome of the
/// blurred zoom; absent column: plains.
fn biome_color(
    view: &crate::environ::WorldView<'_>,
    (palette, rad, plains): &(std::sync::Arc<rc_mesh::blocks::BiomePalette>, i32, u32),
    kind: u8,
    at: [i32; 3],
) -> u32 {
    use rc_mesh::blocks::{SWAMP_GRASS, swamp_grass};
    let k = usize::from(kind.saturating_sub(rc_data::tint::GRASS)).min(3);
    let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
    for z in at[2] - rad..=at[2] + rad {
        for x in at[0] - rad..=at[0] + rad {
            let id = view.biome_full(x, at[1], z, *plains);
            let mut c = palette.get(id as usize).map_or(0xff_ffff, |c| c[k]);
            if c == SWAMP_GRASS {
                c = swamp_grass(x, z);
            }
            r += (c >> 16) & 255;
            g += (c >> 8) & 255;
            b += c & 255;
        }
    }
    let n = ((2 * rad + 1) * (2 * rad + 1)) as u32;
    ((r / n) << 16) | ((g / n) << 8) | (b / n)
}

fn rgb_of(c: u32) -> [f32; 3] {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0]
}

fn norm_id(id: &str) -> String {
    let (ns, path) = rc_assets::source::split_id(id);
    format!("{ns}:{path}")
}

impl ParticleAssets {
    /// To call before freeing the resources: particle images, definitions, "particle" texture
    /// of each block, useful item sprites.
    pub fn load(
        assets: &rc_assets::Assets,
        data: &rc_data::VersionData,
        models: &rc_assets::BlockModels,
        items: &rc_assets::item::ItemModels,
    ) -> Self {
        let t = Instant::now();
        // Sprites: missing, then `textures/particle/*.png`, then item sprites.
        let mut images: Vec<Image> = vec![Image::missing(16)];
        let mut by_name: HashMap<String, u16> = HashMap::new();
        let prefix = "assets/minecraft/textures/particle/";
        for path in assets.list(prefix, ".png") {
            let name = path.trim_start_matches(prefix).trim_end_matches(".png");
            // Vibration signal: whole animated strip (one frame per tick, chosen at render by the common clock).
            let whole = name == "vibration";
            let Some(img) = assets.image(&path).map(|i| if whole { i } else { i.first_frame() }) else {
                continue;
            };
            by_name.insert(format!("minecraft:{name}"), images.len() as u16);
            images.push(img);
        }
        let mut sets = vec![Vec::new(); KINDS.len()];
        for &(k, name) in KINDS {
            let Some(def) = assets
                .read(&format!("assets/minecraft/particles/{name}.json"))
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            else {
                continue;
            };
            let list = def.get("textures").and_then(Value::as_array).cloned().unwrap_or_default();
            sets[k as usize] =
                list.iter().filter_map(Value::as_str).map(|s| by_name.get(&norm_id(s)).copied().unwrap_or(0)).collect();
        }
        // Item shards: "particle" sprite of each layer of the item's model, evaluated like
        // an item placed on the ground without carrier, default stack.
        let sprite_of: HashMap<&str, usize> =
            items.sprite_names.iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
        let item_names = data.registry("minecraft:item").map(|r| r.entries.clone()).unwrap_or_default();
        let mut placed: HashMap<usize, u16> = HashMap::new();
        let mut item_sprites = Vec::with_capacity(item_names.len());
        let (mut layers, mut key) = (Vec::new(), Vec::new());
        for (i, n) in item_names.iter().enumerate() {
            let model = data.items.get(i).map_or(n.as_str(), |d| d.model.as_str());
            layers.clear();
            items.layers(n, model, &GroundItem, &GroundItem, 0, &mut layers, &mut key);
            let list: Vec<u16> = layers
                .iter()
                .map(|l| match items.baked(l.model) {
                    Some(b) => *placed.entry(b.particle as usize).or_insert_with(|| {
                        images.push(items.sprites[b.particle as usize].clone().first_frame());
                        (images.len() - 1) as u16
                    }),
                    None => 0,
                })
                .collect();
            item_sprites.push(list);
        }
        let item_id = |name: &str| {
            item_names
                .iter()
                .position(|n| n == name)
                .and_then(|i| item_sprites.get(i))
                .and_then(|l| l.first().copied())
                .unwrap_or(0)
        };
        let (snowball, slime, cobweb) =
            (item_id("minecraft:snowball"), item_id("minecraft:slime_ball"), item_id("minecraft:cobweb"));
        let tex_index: HashMap<&str, u32> =
            models.texture_names.iter().enumerate().map(|(i, n)| (n.as_str(), i as u32)).collect();
        // Markers: blocks whose "particle" texture is not a block texture (barrier,
        // light, structure void); variant of each state, item sprite copied.
        let mut markers = vec![0u16; data.states.len()];
        for b in &data.blocks {
            // Only the blocks without faces (their "particle" texture is not on the GPU).
            let faces = models
                .states
                .get(b.default_state as usize)
                .is_some_and(|s| s.parts.iter().any(|p| p.options.iter().any(|o| o.2 > 0)));
            if faces {
                continue;
            }
            for s in b.first_state..b.first_state + b.state_count {
                // "Particle" texture of the state's model (merged pack definitions).
                let Some(tex) = models.particle(s) else { continue };
                if let Some(&i) = sprite_of.get(tex) {
                    markers[s as usize] = *placed.entry(i).or_insert_with(|| {
                        images.push(items.sprites[i].clone().first_frame());
                        (images.len() - 1) as u16
                    });
                }
            }
        }
        let translucent = images.iter().map(|i| i.transparency(0.0, 0.0, 1.0, 1.0).translucent).collect();
        let (image, rects) = pack(&images);

        // Blocks: "particle" texture of the state's model, otherwise that of the first face (the
        // tint and colour of the fragments are per-state data, `rc_data`).
        // Per block: one part per distinct "particle" texture of its states (that of the model of
        // each state, like the game; a lit furnace or a pack that covers only one state
        // differ), and the index of each state in these parts.
        let per_block: Vec<(Vec<BlockPart>, Vec<u16>)> = data
            .blocks
            .par_iter()
            .map(|b| {
                let air = matches!(
                    b.name.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" | "minecraft:moving_piston"
                );
                let state = models.states.get(b.default_state as usize);
                let first_quad = state
                    .and_then(|s| s.parts.first())
                    .and_then(|p| p.options.first())
                    .filter(|o| o.2 > 0)
                    .map(|o| &models.quads[o.1 as usize]);
                let mut parts: Vec<BlockPart> = Vec::new();
                let mut index = Vec::with_capacity(b.state_count as usize);
                for s in b.first_state..b.first_state + b.state_count {
                    let particle = models.particle(s).and_then(|n| tex_index.get(n).copied());
                    let tex = particle.or_else(|| first_quad.map(|q| q.texture));
                    let key = tex.unwrap_or(u32::MAX);
                    let i = parts.iter().position(|p| p.tex == key).unwrap_or_else(|| {
                        parts.push(BlockPart {
                            tex: key,
                            none: air || tex.is_none(),
                            invisible: !air && first_quad.is_none(),
                        });
                        parts.len() - 1
                    });
                    index.push(i as u16);
                }
                (parts, index)
            })
            .collect();
        let mut blocks = Vec::new();
        let mut state_block = vec![0u16; data.states.len()];
        for (b, (parts, index)) in data.blocks.iter().zip(per_block) {
            let base = blocks.len() as u16;
            for (k, i) in index.into_iter().enumerate() {
                if let Some(slot) = state_block.get_mut(b.first_state as usize + k) {
                    *slot = base + i;
                }
            }
            blocks.extend(parts);
        }
        eprintln!(
            "particles: {} sprites ({} × {} px), {} types, in {:.0} ms",
            rects.len(),
            image.width,
            image.height,
            sets.iter().filter(|s| !s.is_empty()).count(),
            t.elapsed().as_secs_f64() * 1e3
        );
        ParticleAssets {
            rects,
            translucent,
            sets,
            blocks,
            state_block,
            items: item_sprites,
            snowball,
            slime,
            cobweb,
            block_size: models.texture_size as f32,
            image: Some(image),
            markers,
        }
    }

    fn block(&self, state: u32) -> Option<&BlockPart> {
        self.state_block.get(state as usize).and_then(|&b| self.blocks.get(b as usize))
    }

    fn set(&self, k: Kind) -> &[u16] {
        self.sets.get(k as usize).map_or(&[], Vec::as_slice)
    }
}

/// Packing into shelves (tallest first) in a sheet 1024 pixels wide.
fn pack(images: &[Image]) -> (Image, Vec<[f32; 4]>) {
    const W: u32 = 1024;
    let mut order: Vec<usize> = (0..images.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(images[i].height));
    let mut rects = vec![[0f32; 4]; images.len()];
    let (mut x, mut y, mut row) = (0u32, 0u32, 0u32);
    for &i in &order {
        let (w, h) = (images[i].width.min(W), images[i].height);
        if x + w > W {
            x = 0;
            y += row;
            row = 0;
        }
        rects[i] = [x as f32, y as f32, w as f32, h as f32];
        x += w;
        row = row.max(h);
    }
    let height = (y + row).max(1).next_power_of_two();
    let mut rgba = vec![0u8; (W * height * 4) as usize];
    for (img, r) in images.iter().zip(&rects) {
        let (x0, y0, w) = (r[0] as usize, r[1] as usize, r[2] as usize);
        for row in 0..img.height as usize {
            let src = &img.rgba[row * img.width as usize * 4..][..w * 4];
            let at = ((y0 + row) * W as usize + x0) * 4;
            rgba[at..at + w * 4].copy_from_slice(src);
        }
    }
    (Image { width: W, height, rgba }, rects)
}

// ---------- World ----------

/// The client world as seen by the particles: states, collision shapes, fluids, light.
pub struct TickWorld<'a> {
    pub grid: &'a ChunkGrid,
    pub min_y: i32,
    pub height: i32,
    pub phys: &'a rc_physics::Tables,
    /// Local player: position (feet) and velocity, for the cloud (§ 3, cloud).
    pub player: Option<([f64; 3], [f64; 3])>,
    /// Eyes of the local player when aiming with a spyglass in first person: spells within
    /// 3 blocks become invisible.
    pub scope: Option<[f64; 3]>,
}

/// Point within 3 blocks (inclusive) of the eyes of the player aiming with a spyglass.
fn near_scope(scope: Option<[f64; 3]>, p: [f64; 3]) -> bool {
    scope.is_some_and(|e| (0..3).map(|i| (e[i] - p[i]).powi(2)).sum::<f64>() <= 9.0)
}

thread_local! {
    /// Sweep and cell boxes, reused from one call to the next.
    static SCRATCH: RefCell<(Vec<Aabb>, Vec<Aabb>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

fn light_nibble(layers: &[rc_world::LightLayer], min_y: i32, x: i32, y: i32, z: i32) -> u8 {
    let sy = ((y - min_y) >> 4) + 1;
    let Some(Some(layer)) = usize::try_from(sy).ok().and_then(|i| layers.get(i)) else { return 0 };
    let i = (((y & 15) as usize) << 8) | (((z & 15) as usize) << 4) | ((x & 15) as usize);
    (layer[i >> 1] >> ((i & 1) * 4)) & 15
}

impl TickWorld<'_> {
    /// Block state; chunk absent: nothing; outside height: air.
    pub(crate) fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let c = self.grid.get(x >> 4, z >> 4)?;
        let ly = y - self.min_y;
        if !(0..self.height).contains(&ly) {
            return Some(0);
        }
        c.block((x & 15) as usize, ly as usize, (z & 15) as usize)
    }

    /// Light (block, sky) of the block; chunk absent: sky 15, block 0.
    fn light(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let Some(c) = self.grid.get(x >> 4, z >> 4) else { return (0, 15) };
        if y >= self.min_y + self.height {
            return (0, 15);
        }
        (light_nibble(&c.block_light, self.min_y, x, y, z), light_nibble(&c.sky_light, self.min_y, x, y, z))
    }

    /// Fluid of the block and its height.
    pub(crate) fn fluid(&self, x: i32, y: i32, z: i32) -> (Fluid, f32) {
        let s = self.state(x, y, z).unwrap_or(0);
        let f = self.phys.fluids.fluid(s);
        if f == Fluid::None {
            return (f, 0.0);
        }
        let above = self.state(x, y + 1, z).unwrap_or(0);
        (f, self.phys.fluids.height_at(s, above))
    }

    /// Top of the block's collision shape under the local point (x, z).
    fn shape_top(&self, x: i32, y: i32, z: i32, fx: f64, fz: f64) -> f64 {
        let Some(s) = self.state(x, y, z) else { return 0.0 };
        SCRATCH.with_borrow_mut(|(_, cell)| {
            cell.clear();
            self.phys.collision(s, cell);
            cell.iter()
                .filter(|b| b.min[0] <= fx && fx <= b.max[0] && b.min[2] <= fz && fz <= b.max[2])
                .map(|b| b.max[1])
                .fold(0.0, f64::max)
        })
    }

    /// Allowed movement of box `b` (same algorithm as the entities, without entities).
    fn collide(&self, b: &Aabb, d: [f64; 3]) -> [f64; 3] {
        use rc_physics::collide::EPS;
        let zone = b.expand_towards(d);
        SCRATCH.with_borrow_mut(|(out, cell)| {
            out.clear();
            let lo = [0, 1, 2].map(|a| (zone.min[a] - EPS).floor() as i32 - 1);
            let hi = [0, 1, 2].map(|a| (zone.max[a] + EPS).floor() as i32 + 1);
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    for x in lo[0]..=hi[0] {
                        let edges = u8::from(x == lo[0] || x == hi[0])
                            + u8::from(y == lo[1] || y == hi[1])
                            + u8::from(z == lo[2] || z == hi[2]);
                        if edges >= 2 {
                            continue;
                        }
                        let Some(s) = self.state(x, y, z) else { continue };
                        if s == 0 {
                            continue;
                        }
                        cell.clear();
                        let oversized = self.phys.collision(s, cell);
                        if edges == 1 && !oversized {
                            continue;
                        }
                        for local in cell.iter() {
                            let bx = local.offset(f64::from(x), f64::from(y), f64::from(z));
                            if bx.intersects(&zone) {
                                out.push(bx);
                            }
                        }
                    }
                }
            }
            if out.is_empty() { d } else { rc_physics::collide::sweep(out, b, d) }
        })
    }
}

impl rc_physics::World for TickWorld<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        TickWorld::state(self, x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        TickWorld::state(self, x, y, z).is_some_and(|s| self.phys.collision(s, out))
    }

    fn min_y(&self) -> i32 {
        self.min_y
    }
}

/// Harmful gas of potent sulfur: per state, water block, water source (placed water or waterlogged block),
/// air, empty collision shape.
pub struct GasTables {
    water_block: Vec<bool>,
    water_source: Vec<bool>,
    open: Vec<bool>,
}

impl GasTables {
    pub fn new(d: &rc_data::VersionData) -> Self {
        let water = d.blocks.iter().position(|b| b.name == "minecraft:water");
        let water_fluid: Vec<bool> = d.fluids.iter().map(|n| n == "minecraft:water").collect();
        GasTables {
            water_block: d.states.iter().map(|s| Some(usize::from(s.block)) == water).collect(),
            water_source: d
                .states
                .iter()
                .map(|s| {
                    s.has(rc_data::flags::FLUID_SOURCE)
                        && water_fluid.get(usize::from(s.fluid)).copied().unwrap_or(false)
                })
                .collect(),
            open: d.states.iter().map(|s| s.has(rc_data::flags::AIR) || d.shape(s.collision).is_empty()).collect(),
        }
    }

    fn at(v: &[bool], s: u32) -> bool {
        v.get(s as usize).copied().unwrap_or(false)
    }

    /// The gas passes through: air, water block or empty collision shape.
    fn passable(&self, s: u32) -> bool {
        Self::at(&self.open, s) || Self::at(&self.water_block, s)
    }

    /// Block from which the gas comes out above the sulfur `origin`: go up (5 blocks at most) as long as
    /// the block is a water source that is a water block or lets through; the first other
    /// block is the exit if it lets through, otherwise no gas.
    pub fn source(&self, state: impl Fn(i32, i32, i32) -> u32, origin: [i32; 3]) -> Option<[i32; 3]> {
        for y in origin[1] + 1..=origin[1] + 5 {
            let s = state(origin[0], y, origin[2]);
            let inside_water = Self::at(&self.water_source, s) && self.passable(s);
            if !inside_water {
                return self.passable(s).then_some([origin[0], y, origin[2]]);
            }
        }
        None
    }

    /// The gas coming out of `source` reaches `pos`: passable block, at most 3 blocks from the centre
    /// of the exit, water source just below and clear view below the surface.
    fn reaches(&self, w: &TickWorld<'_>, source: [i32; 3], pos: [f64; 3]) -> bool {
        let b = pos.map(|v| v.floor() as i32);
        if !self.passable(w.state(b[0], b[1], b[2]).unwrap_or(0)) {
            return false;
        }
        let c = source.map(|v| f64::from(v) + 0.5);
        if (0..3).map(|i| (pos[i] - c[i]).powi(2)).sum::<f64>() > 9.0 {
            return false;
        }
        let below = [pos[0], pos[1] - 1.0, pos[2]];
        let wb = below.map(|v| v.floor() as i32);
        let water = w.state(wb[0], wb[1], wb[2]).is_some_and(|s| Self::at(&self.water_source, s));
        water && rc_physics::body::clip_blocks(w, [c[0], c[1] - 1.0, c[2]], below).is_none()
    }
}

// ---------- Particle ----------

/// Child particle requested during a tick (added afterwards by the engine, with filter).
type Child = (Kind, [f64; 3], [f64; 3]);

#[derive(Clone, Debug)]
struct P {
    kind: Kind,
    pos: [f64; 3],
    old: [f64; 3],
    vel: [f64; 3],
    /// Bottom corner of the box and its dimensions (width, height).
    bmin: [f64; 3],
    bw: f64,
    bh: f64,
    age: i32,
    life: i32,
    gravity: f32,
    friction: f32,
    collide: bool,
    on_ground: bool,
    stopped: bool,
    /// Acceleration when the vertical movement is blocked ("on the ceiling").
    ceil: bool,
    removed: bool,
    translucent: bool,
    size: f32,
    roll: f32,
    old_roll: f32,
    roll_speed: f32,
    rgba: [f32; 4],
    alpha0: f32,
    fade: Option<[f32; 3]>,
    sprite: u16,
    /// Block texture + 1 (block fragments), 0 otherwise.
    block_tex: u32,
    /// Sub-rectangle of the sprite in fractions (u0, v0, u1, v1).
    uv: [f32; 4],
    /// Start and parameter (portal, enchantment).
    start: [f64; 3],
    param: [f64; 3],
    rng: Rng,
    child: Option<Child>,
    /// Sound requested at the end of the tick (name, volume), played at the particle's position.
    sound: Option<(&'static str, f32)>,
    /// Trail spark requested this tick (firework).
    spark_child: bool,
    /// Biome tint to apply on entering the queue (kind | offset << 8, 0: none):
    /// the current colour is multiplied by that of the biome at the offset creation block.
    biome: u32,
    /// Vibration signal that follows an entity: id and height above the feet.
    vib_target: Option<(i32, f32)>,
}

/// Tint extracted from a state: kind (`rc_data::tint::*`) and value (ARGB colour, or offset
/// of the position consulted, `(d+1)` per axis on 2 bits).
type StateTint = (u8, u32);

/// What the factory reads in addition to the resources.
#[derive(Clone, Copy)]
struct Make<'a> {
    a: &'a ParticleAssets,
    /// Per state: tint of the block fragments, colour of the falling dust.
    terrain: &'a [StateTint],
    dust: &'a [StateTint],
    /// Eyes of the player aiming with a spyglass (spells invisible within 3 blocks).
    scope: Option<[f64; 3]>,
}

/// Factory context taken from the engine, field by field (the spell generator remains
/// borrowable separately).
macro_rules! make {
    ($s:ident) => {
        Make { a: &$s.assets, terrain: &$s.terrain, dust: &$s.dust, scope: $s.scope }
    };
}

impl Make<'_> {
    /// Applies a state tint to `q` (colour `base` × tint): fixed right away, biome
    /// on entering the queue; none: `base`.
    fn tint(q: &mut P, t: Option<&StateTint>, base: f32) {
        use rc_data::tint;
        q.gray(base);
        match t.copied() {
            Some((tint::CONSTANT, c)) => q.color(rgb_of(c).map(|v| v * base)),
            Some((k @ (tint::GRASS | tint::FOLIAGE | tint::DRY_FOLIAGE | tint::WATER), off)) => {
                q.biome = u32::from(k) | (off & 63) << 8;
            }
            _ => {}
        }
    }
}

fn len3(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Frame `clock mod n` of a vertical animated strip (square frames, one per tick) as a
/// sub-rectangle of sprite `r` (pixels: x, y, width, height).
fn strip_frame(r: [f32; 4], clock: u32) -> [f32; 4] {
    let n = if r[2] > 0.0 { (r[3] / r[2]).round().max(1.0) as u32 } else { 1 };
    let f = (clock % n) as f32;
    let n = n as f32;
    [0.0, f / n, 1.0, (f + 1.0) / n]
}

impl P {
    /// Base construction (§ 2.2).
    /// Vibration signal: heading (roll) and pitch (`param[0]`, previous in `param[1]`)
    /// from the offset to the target.
    fn face_target(&mut self) {
        let d = [self.pos[0] - self.start[0], self.pos[1] - self.start[1], self.pos[2] - self.start[2]];
        self.old_roll = self.roll;
        self.roll = d[0].atan2(d[2]) as f32;
        self.param[1] = self.param[0];
        self.param[0] = f64::from(d[1].atan2((d[0] * d[0] + d[2] * d[2]).sqrt()) as f32);
    }

    /// Interpolated pitch and sway, quantised on 14 bits each (bits 14 to 27 and 0 to 13
    /// of `tex`): error of at most 1e-4 rad, imperceptible.
    fn vib_angles(&self, pt: f32) -> u32 {
        let pitch = self.param[1] as f32 + (self.param[0] - self.param[1]) as f32 * pt;
        let sway = ((self.age as f32 + pt - std::f32::consts::TAU) * 0.05).sin() * 2.0;
        let q = |v: f32, lo: f32, hi: f32| (((v - lo) / (hi - lo)).clamp(0.0, 1.0) * 16383.0).round() as u32;
        let half = std::f32::consts::FRAC_PI_2;
        q(pitch, -half, half) << 14 | q(sway, -2.0, 2.0)
    }

    fn base(kind: Kind, pos: [f64; 3], mut rng: Rng) -> P {
        let life = (4.0 / (rng.f() * 0.9 + 0.1)) as i32;
        let size = 0.1 * (rng.f() * 0.5 + 0.5) * 2.0;
        P {
            kind,
            pos,
            old: pos,
            vel: [0.0; 3],
            bmin: [pos[0] - 0.1, pos[1], pos[2] - 0.1],
            bw: 0.2,
            bh: 0.2,
            age: 0,
            life,
            gravity: 0.0,
            friction: 0.98,
            collide: true,
            on_ground: false,
            stopped: false,
            ceil: false,
            removed: false,
            translucent: false,
            size,
            roll: 0.0,
            old_roll: 0.0,
            roll_speed: 0.0,
            rgba: [1.0; 4],
            alpha0: 1.0,
            fade: None,
            sprite: 0,
            block_tex: 0,
            uv: [0.0, 0.0, 1.0, 1.0],
            start: pos,
            param: [0.0; 3],
            rng,
            child: None,
            sound: None,
            spark_child: false,
            biome: 0,
            vib_target: None,
        }
    }

    fn r(&mut self) -> f32 {
        self.rng.f()
    }

    fn rd(&mut self) -> f64 {
        self.rng.d()
    }

    /// "With velocity" construction of parameter `p`: the norm does not depend on `p`.
    fn with_vel(&mut self, p: [f64; 3]) {
        let mut v = [0.0; 3];
        for (o, c) in v.iter_mut().zip(p) {
            *o = c + (self.rd() * 2.0 - 1.0) * 0.4;
        }
        let s = (self.rd() + self.rd() + 1.0) * 0.15;
        let n = len3(v).max(1e-9);
        self.vel = v.map(|c| c / n * s * 0.4);
        self.vel[1] += 0.1;
    }

    fn power(&mut self, k: f64) {
        self.vel[0] *= k;
        self.vel[1] = (self.vel[1] - 0.1) * k + 0.1;
        self.vel[2] *= k;
    }

    /// New box size, re-centred on the centre of the box (not on the position, which may
    /// have been offset alone), bottom unchanged; same size: nothing.
    fn set_size(&mut self, w: f64, h: f64) {
        if w == self.bw && h == self.bh {
            return;
        }
        let (x, z) = (self.bmin[0] + (self.bw - w) / 2.0, self.bmin[2] + (self.bw - w) / 2.0);
        self.bw = w;
        self.bh = h;
        self.bmin = [x, self.bmin[1], z];
    }

    fn scale(&mut self, e: f32) {
        self.size *= e;
        self.set_size(0.2 * f64::from(e), 0.2 * f64::from(e));
    }

    fn set_pos(&mut self, p: [f64; 3]) {
        self.pos = p;
        self.bmin = [p[0] - self.bw / 2.0, p[1], p[2] - self.bw / 2.0];
    }

    fn gray(&mut self, c: f32) {
        self.rgba[0] = c;
        self.rgba[1] = c;
        self.rgba[2] = c;
    }

    fn color(&mut self, c: [f32; 3]) {
        self.rgba[..3].copy_from_slice(&c);
    }

    /// Sub-square of 1/4 of the sprite, u inverted (block and item fragments).
    fn chunk_uv(&mut self) {
        let (uo, vo) = (self.r() * 3.0, self.r() * 3.0);
        self.uv = [(uo + 1.0) / 4.0, vo / 4.0, uo / 4.0, (vo + 1.0) / 4.0];
    }

    fn pick_random(&mut self, a: &ParticleAssets) {
        let set = a.set(self.kind);
        if !set.is_empty() {
            self.sprite = set[self.rng.int(set.len() as u32) as usize];
        }
    }

    fn pick_age(&mut self, a: &ParticleAssets) {
        let set = a.set(self.kind);
        if !set.is_empty() {
            let i = (self.age as usize * (set.len() - 1)) / self.life.max(1) as usize;
            self.sprite = set[i.min(set.len() - 1)];
        }
    }

    fn aabb(&self) -> Aabb {
        Aabb { min: self.bmin, max: [self.bmin[0] + self.bw, self.bmin[1] + self.bh, self.bmin[2] + self.bw] }
    }

    /// Movement (§ 2.2): collisions if active, frozen when a fall is stopped.
    fn move_by(&mut self, d: [f64; 3], w: Option<&TickWorld<'_>>) {
        if self.stopped {
            return;
        }
        let mut m = d;
        if let Some(w) = w.filter(|_| self.collide)
            && d != [0.0; 3]
            && d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 10_000.0
        {
            m = w.collide(&self.aabb(), d);
        }
        if m != [0.0; 3] {
            for (b, v) in self.bmin.iter_mut().zip(m) {
                *b += v;
            }
            self.pos = [self.bmin[0] + self.bw / 2.0, self.bmin[1], self.bmin[2] + self.bw / 2.0];
        }
        if d[1].abs() >= 1.0e-5 && m[1].abs() < 1.0e-5 {
            self.stopped = true;
        }
        self.on_ground = d[1] != m[1] && d[1] < 0.0;
        if d[0] != m[0] {
            self.vel[0] = 0.0;
        }
        if d[2] != m[2] {
            self.vel[2] = 0.0;
        }
    }

    /// Standard tick; returns false if the particle dies.
    fn standard(&mut self, w: Option<&TickWorld<'_>>) -> bool {
        self.old = self.pos;
        let age = self.age;
        self.age += 1;
        if age >= self.life {
            self.removed = true;
            return false;
        }
        self.vel[1] -= 0.04 * f64::from(self.gravity);
        self.move_by(self.vel, w);
        if self.ceil && self.pos[1] == self.old[1] {
            self.vel[0] *= 1.1;
            self.vel[2] *= 1.1;
        }
        let f = f64::from(self.friction);
        self.vel = self.vel.map(|v| v * f);
        if self.on_ground {
            self.vel[0] *= 0.7;
            self.vel[2] *= 0.7;
        }
        true
    }

    /// Counted-down life (drips, rain, bubbles…): returns false on expiry.
    fn count_down(&mut self) -> bool {
        self.old = self.pos;
        let l = self.life;
        self.life -= 1;
        if l <= 0 {
            self.removed = true;
            return false;
        }
        true
    }

    fn in_own_fluid(&self, w: Option<&TickWorld<'_>>, want: Fluid) -> bool {
        let Some(w) = w else { return false };
        let b = self.pos.map(|v| v.floor() as i32);
        let (f, h) = w.fluid(b[0], b[1], b[2]);
        f == want && self.pos[1] < f64::from(b[1]) + f64::from(h)
    }

    /// Flags of a firework spark (`SPARK_*`), kept in `param[0]`.
    fn spark_flags(&self) -> u8 {
        self.param[0] as u8
    }

    /// Twinkling spark: hidden a third of the time after the first third of its life.
    fn hidden(&self) -> bool {
        self.kind == K::Firework
            && self.spark_flags() & SPARK_TWINKLE != 0
            && self.age >= self.life / 3
            && ((self.age + self.life) / 3) % 2 != 0
    }

    /// Falling leaf (300 ticks): wind that pushes it aside and/or swirl, accelerated rotation.
    /// `param` = (x offset, z offset, swirl period), `start` = (wind, flags: 1
    /// swirl, 2 offset, rotation acceleration), `roll_speed` = rotation speed.
    fn tick_leaf(&mut self, w: Option<&TickWorld<'_>>) {
        self.old = self.pos;
        let l = self.life;
        self.life -= 1;
        if l <= 0 {
            self.removed = true;
            return;
        }
        let alive = (300 - self.life) as f32;
        let rel = (alive / 300.0).min(1.0);
        let (wind, flags) = (self.start[0], self.start[1] as u8);
        let (mut xa, mut za) = (0.0f64, 0.0f64);
        if flags & 2 != 0 {
            let k = f64::from(rel).powf(1.25);
            xa += self.param[0] * k;
            za += self.param[1] * k;
        }
        if flags & 1 != 0 {
            let r = f64::from(rel);
            xa += r * (r * self.param[2]).cos() * wind;
            za += r * (r * self.param[2]).sin() * wind;
        }
        self.vel[0] += xa * f64::from(0.0025f32);
        self.vel[2] += za * f64::from(0.0025f32);
        self.vel[1] -= f64::from(self.gravity);
        self.roll_speed += self.start[2] as f32 / 20.0;
        self.old_roll = self.roll;
        self.roll += self.roll_speed / 20.0;
        self.move_by(self.vel, w);
        if self.on_ground || self.life < 299 && (self.vel[0] == 0.0 || self.vel[2] == 0.0) {
            self.removed = true;
        }
    }

    fn tick(&mut self, a: &ParticleAssets, w: Option<&TickWorld<'_>>) {
        match self.kind {
            K::DrippingWater
            | K::DrippingLava
            | K::DrippingDripstoneWater
            | K::DrippingDripstoneLava
            | K::DrippingHoney
            | K::DrippingObsidianTear => {
                // Hanging drip: on expiry, the falling drip takes over.
                self.old = self.pos;
                if matches!(self.kind, K::DrippingLava | K::DrippingDripstoneLava) {
                    let n = self.life as f32;
                    self.color([1.0, 16.0 / (40.0 - n + 16.0), 4.0 / (40.0 - n + 8.0)]);
                }
                let l = self.life;
                self.life -= 1;
                if l <= 0 {
                    self.removed = true;
                    let next = match self.kind {
                        K::DrippingWater => K::FallingWater,
                        K::DrippingLava => K::FallingLava,
                        K::DrippingDripstoneWater => K::FallingDripstoneWater,
                        K::DrippingHoney => K::FallingHoney,
                        K::DrippingObsidianTear => K::FallingObsidianTear,
                        _ => K::FallingDripstoneLava,
                    };
                    self.child = Some((next, self.pos, self.vel));
                    return;
                }
                self.vel[1] -= f64::from(self.gravity);
                self.move_by(self.vel, w);
                self.vel = self.vel.map(|v| v * 0.02 * 0.98);
                if let Some(fluid) = self.kind.drip_fluid()
                    && self.in_own_fluid(w, fluid)
                {
                    self.removed = true;
                }
            }
            K::FallingWater
            | K::FallingLava
            | K::LandingLava
            | K::FallingDripstoneWater
            | K::FallingDripstoneLava
            | K::FallingHoney
            | K::LandingHoney
            | K::FallingObsidianTear
            | K::LandingObsidianTear
            | K::FallingNectar
            | K::FallingSporeBlossom => {
                if !self.count_down() {
                    return;
                }
                self.vel[1] -= f64::from(self.gravity);
                self.move_by(self.vel, w);
                let landing = matches!(self.kind, K::LandingLava | K::LandingHoney | K::LandingObsidianTear);
                if self.on_ground && !landing {
                    self.removed = true;
                    // Simple fall (nectar, spores): nothing on impact.
                    let next = match self.kind {
                        K::FallingWater | K::FallingDripstoneWater => Some(K::Splash),
                        K::FallingLava | K::FallingDripstoneLava => Some(K::LandingLava),
                        K::FallingHoney => Some(K::LandingHoney),
                        K::FallingObsidianTear => Some(K::LandingObsidianTear),
                        _ => None,
                    };
                    if let Some(next) = next {
                        self.child = Some((next, self.pos, [0.0; 3]));
                    }
                    // Stalactite and honey drips: drip sound, volume drawn between 0.3 and 1.
                    let name = match self.kind {
                        K::FallingDripstoneWater => Some("minecraft:block.pointed_dripstone.drip_water"),
                        K::FallingDripstoneLava => Some("minecraft:block.pointed_dripstone.drip_lava"),
                        K::FallingHoney => Some("minecraft:block.beehive.drip"),
                        _ => None,
                    };
                    if let Some(name) = name {
                        let v = 0.3 + self.r() * 0.7;
                        self.sound = Some((name, v));
                    }
                    return;
                }
                self.vel = self.vel.map(|v| v * 0.98);
                if let Some(fluid) = self.kind.drip_fluid()
                    && self.in_own_fluid(w, fluid)
                {
                    self.removed = true;
                }
            }
            K::CherryLeaves
            | K::PaleOakLeaves
            | K::RedPoplarLeaves
            | K::OrangePoplarLeaves
            | K::YellowPoplarLeaves
            | K::TintedLeaves => self.tick_leaf(w),
            K::BubbleColumnUp => {
                if self.standard(w)
                    && let Some(w) = w
                {
                    let b = self.pos.map(|v| v.floor() as i32);
                    if w.fluid(b[0], b[1], b[2]).0 != Fluid::Water {
                        self.removed = true;
                    }
                }
            }
            K::CurrentDown => {
                // Descending swirl: angle in `param[0]`, no collision.
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                let angle = self.param[0] as f32;
                self.vel[0] = (self.vel[0] + f64::from(0.6 * angle.cos())) * 0.07;
                self.vel[2] = (self.vel[2] + f64::from(0.6 * angle.sin())) * 0.07;
                self.move_by(self.vel, w);
                if let Some(w) = w {
                    let b = self.pos.map(|v| v.floor() as i32);
                    if w.fluid(b[0], b[1], b[2]).0 != Fluid::Water || self.on_ground {
                        self.removed = true;
                    }
                }
                self.param[0] = f64::from(angle + 0.08);
            }
            K::ReversePortal => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                let k = f64::from(self.age as f32 / self.life as f32);
                self.pos =
                    [self.pos[0] + self.vel[0] * k, self.pos[1] + self.vel[1] * k, self.pos[2] + self.vel[2] * k];
            }
            K::Firefly => {
                if !self.standard(w) {
                    return;
                }
                let b = self.pos.map(|v| v.floor() as i32);
                if let Some(w) = w
                    && !w.state(b[0], b[1], b[2]).is_none_or(|s| w.phys.is_air(s))
                {
                    self.removed = true;
                    return;
                }
                self.rgba[3] = fade(progress(self.age as f32, self.life), 0.3, 0.5);
                if self.r() > 0.95 || self.age == 1 {
                    let mut v = || f64::from(-0.05 + 0.1 * self.rng.f());
                    self.vel = [v(), v(), v()];
                }
            }
            K::SulfurBubbles => {
                // `start[1]`: start y; `param`: end y, previous y, start size.
                if !self.standard(w) {
                    return;
                }
                let b = self.pos.map(|v| v.floor() as i32);
                let in_source = w.is_none_or(|w| {
                    w.state(b[0], b[1], b[2]).is_some_and(|s| {
                        w.phys.fluids.fluid(s) == Fluid::Water
                            && w.phys.fluids.source.get(s as usize).copied().unwrap_or(false)
                    })
                });
                if !in_source || self.pos[1] >= self.param[0] || self.pos[1] <= self.param[1] {
                    self.removed = true;
                }
                let mut wiggle = || {
                    let a = f64::from(self.rng.f() * 0.003);
                    if self.rng.int(2) == 0 { a * 0.5 } else { -a * 0.5 }
                };
                self.vel[0] += wiggle();
                self.vel[2] += wiggle();
                self.move_by([self.vel[0], 0.0, self.vel[2]], w);
                let travel = ((self.pos[1] - self.start[1]) / (self.param[0] - self.start[1])) as f32;
                let s0 = self.param[2] as f32;
                self.size = s0 + travel * (0.15 - s0);
                self.param[1] = self.pos[1];
            }
            K::DragonBreath => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                self.pick_age(a);
                self.move_by(self.vel, w);
                if self.pos[1] == self.old[1] {
                    self.vel[0] *= 1.1;
                    self.vel[2] *= 1.1;
                }
                self.vel[0] *= 0.96;
                self.vel[2] *= 0.96;
            }
            K::Vibration => {
                // Brought closer to the target (`start`) by a fraction 1/remaining ticks; the last
                // tick (0 remaining) sends it to infinity in the game: it disappears.
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                let remaining = self.life - self.age;
                if age >= self.life || remaining <= 0 {
                    self.removed = true;
                    return;
                }
                let k = 1.0 / f64::from(remaining);
                for i in 0..3 {
                    self.pos[i] += k * (self.start[i] - self.pos[i]);
                }
                self.face_target();
            }
            K::Shriek => {
                // Delay before appearing (ticks), then slow rise over 30 ticks.
                if self.param[0] > 0.0 {
                    self.param[0] -= 1.0;
                } else if self.standard(w) {
                    self.after_standard(a, w);
                }
            }
            K::OminousSpawning => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                let n = self.age as f32 / self.life as f32;
                let k = f64::from(1.0 - n);
                let (s, p) = (self.start, self.param);
                self.pos = [s[0] + p[0] * k, s[1] + p[1] * k, s[2] + p[2] * k];
                // From blue-violet (0x45aefe) to white.
                let from = [0x45 as f32, 0xae as f32, 0xfe as f32];
                self.rgba[0] = (from[0] + (255.0 - from[0]) * n) / 255.0;
                self.rgba[1] = (from[1] + (255.0 - from[1]) * n) / 255.0;
                self.rgba[2] = (from[2] + (255.0 - from[2]) * n) / 255.0;
            }
            K::Rain | K::Splash => {
                if !self.count_down() {
                    return;
                }
                self.vel[1] -= f64::from(self.gravity);
                self.move_by(self.vel, w);
                self.vel = self.vel.map(|v| v * 0.98);
                if self.on_ground {
                    if self.r() < 0.5 {
                        self.removed = true;
                        return;
                    }
                    self.vel[0] *= 0.7;
                    self.vel[2] *= 0.7;
                }
                if let Some(w) = w {
                    let b = self.pos.map(|v| v.floor() as i32);
                    let fx = self.pos[0] - f64::from(b[0]);
                    let fz = self.pos[2] - f64::from(b[2]);
                    let top = w.shape_top(b[0], b[1], b[2], fx, fz).max(f64::from(w.fluid(b[0], b[1], b[2]).1));
                    if top > 0.0 && self.pos[1] < f64::from(b[1]) + top {
                        self.removed = true;
                    }
                }
            }
            K::Bubble => {
                if !self.count_down() {
                    return;
                }
                self.vel[1] += 0.002;
                self.move_by(self.vel, w);
                self.vel = self.vel.map(|v| v * 0.85);
                if let Some(w) = w {
                    let b = self.pos.map(|v| v.floor() as i32);
                    if w.fluid(b[0], b[1], b[2]).0 != Fluid::Water {
                        self.removed = true;
                    }
                }
            }
            K::HappyVillager | K::Composter | K::EggCrack | K::Mycelium | K::Dolphin => {
                if !self.count_down() {
                    return;
                }
                self.move_by(self.vel, w);
                self.vel = self.vel.map(|v| v * 0.99);
            }
            K::Portal => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                let q = f64::from(self.age) / f64::from(self.life);
                let f = 1.0 - (2.0 * q * q - q);
                let (s, p) = (self.start, self.param);
                self.pos = [s[0] + p[0] * f, s[1] + p[1] * f + (1.0 - q), s[2] + p[2] * f];
            }
            K::Enchant | K::Nautilus | K::VaultConnection => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                let k = 1.0 - f64::from(self.age) / f64::from(self.life);
                let e = (1.0 - k).powi(4);
                let (s, p) = (self.start, self.param);
                self.pos = [s[0] + p[0] * k, s[1] + p[1] * k - 1.2 * e, s[2] + p[2] * k];
            }
            K::Fishing => {
                // Wake: 60 − life chooses the frame (4 frames in a loop) before the countdown.
                self.old = self.pos;
                let t = 60 - self.life;
                let l = self.life;
                self.life -= 1;
                if l <= 0 {
                    self.removed = true;
                    return;
                }
                self.vel[1] -= f64::from(self.gravity);
                self.move_by(self.vel, w);
                self.vel = self.vel.map(|v| v * f64::from(0.98f32));
                let set = a.set(self.kind);
                if !set.is_empty() {
                    let i = (t.rem_euclid(4) as usize * (set.len() - 1)) / 4;
                    self.sprite = set[i.min(set.len() - 1)];
                }
            }
            K::Explosion | K::SweepAttack | K::Flash | K::SonicBoom | K::Gust | K::SmallGust => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                } else if self.kind != K::Flash {
                    self.pick_age(a);
                }
            }
            K::CampfireCosySmoke | K::CampfireSignalSmoke => {
                self.old = self.pos;
                let age = self.age;
                self.age += 1;
                if age < self.life && self.rgba[3] > 0.0 {
                    for i in [0, 2] {
                        let d = f64::from(self.r()) / 5000.0;
                        self.vel[i] += if self.r() < 0.5 { d } else { -d };
                    }
                    self.vel[1] -= f64::from(self.gravity);
                    self.move_by(self.vel, w);
                    if self.age >= self.life - 60 && self.rgba[3] > 0.01 {
                        self.rgba[3] -= 0.015;
                    }
                } else {
                    self.removed = true;
                }
            }
            K::FallingDust => {
                self.old = self.pos;
                self.old_roll = self.roll;
                let age = self.age;
                self.age += 1;
                if age >= self.life {
                    self.removed = true;
                    return;
                }
                self.pick_age(a);
                self.roll += std::f32::consts::PI * self.roll_speed * 2.0;
                if self.on_ground {
                    self.old_roll = 0.0;
                    self.roll = 0.0;
                }
                self.move_by(self.vel, w);
                self.vel[1] = (self.vel[1] - 0.003).max(-0.14);
            }
            _ => {
                if self.kind == K::DustPlume {
                    // Gravity and friction fade a little at each tick.
                    self.gravity *= 0.88;
                    self.friction *= 0.92;
                }
                if !self.standard(w) {
                    return;
                }
                self.after_standard(a, w);
            }
        }
    }

    /// Type-specific additions after the standard tick.
    fn after_standard(&mut self, a: &ParticleAssets, w: Option<&TickWorld<'_>>) {
        match self.kind {
            K::Crit | K::EnchantedHit | K::DamageIndicator => {
                self.rgba[1] *= 0.96;
                self.rgba[2] *= 0.9;
            }
            K::Lava => {
                let f = self.age as f32 / self.life as f32;
                if self.r() > f {
                    self.child = Some((K::Smoke, self.pos, self.vel));
                }
            }
            K::Snowflake => {
                self.vel[0] *= 0.95;
                self.vel[1] *= 0.9;
                self.vel[2] *= 0.95;
            }
            k if k.spell() => {
                if near_scope(w.and_then(|w| w.scope), self.pos) {
                    self.rgba[3] = 0.0;
                } else {
                    self.rgba[3] += 0.05 * (self.alpha0 - self.rgba[3]);
                }
            }
            K::Cloud | K::Sneeze => {
                if let Some((p, v)) = w.and_then(|w| w.player) {
                    let d2: f64 = (0..3).map(|i| (p[i] - self.pos[i]).powi(2)).sum();
                    if d2 < 4.0 && self.pos[1] > p[1] {
                        let y = self.pos[1] + (p[1] - self.pos[1]) * 0.2;
                        self.vel[1] += (v[1] - self.vel[1]) * 0.2;
                        self.set_pos([self.pos[0], y, self.pos[2]]);
                    }
                }
            }
            K::NoxiousGas => {
                let half = self.life as f32 / 2.0;
                if self.age as f32 > half {
                    self.rgba[3] = (self.life as f32 - (self.age as f32 - half)) / self.life as f32;
                }
            }
            K::SquidInk | K::GlowSquidInk => {
                let half = self.life / 2;
                if self.age > half {
                    self.rgba[3] = 1.0 - (self.age - half) as f32 / self.life as f32;
                }
                // In the air, the ink falls back.
                let b = self.pos.map(|v| v.floor() as i32);
                if let Some(w) = w
                    && w.state(b[0], b[1], b[2]).is_none_or(|s| w.phys.is_air(s))
                {
                    self.vel[1] -= f64::from(0.0074f32);
                }
            }
            K::TotemOfUndying | K::EndRod | K::Firework => {
                let half = self.life / 2;
                if self.age > half {
                    self.rgba[3] = 1.0 - (self.age - half) as f32 / self.life as f32;
                    if let Some(f) = self.fade {
                        for (c, t) in self.rgba.iter_mut().zip(f) {
                            *c += (t - *c) * 0.2;
                        }
                    }
                }
                // Trail spark: a spark immobile one tick out of two, first half.
                if self.kind == K::Firework
                    && self.spark_flags() & SPARK_TRAIL != 0
                    && self.age < self.life / 2
                    && (self.age + self.life) % 2 == 0
                {
                    self.spark_child = true;
                }
            }
            _ => {}
        }
        if self.kind.animated() {
            self.pick_age(a);
        }
    }

    /// Size and alpha displayed at `t` (fraction of the tick).
    fn render_size(&self, t: f32) -> (f32, f32) {
        let life = self.life.max(1) as f32;
        let s = (self.age as f32 + t) / life;
        let size = match self.kind {
            k if k.grows() => self.size * (s * 32.0).clamp(0.0, 1.0),
            k if k.flame() => self.size * (1.0 - s * s * 0.5),
            K::Lava => self.size * (1.0 - s * s),
            K::Portal => self.size * (1.0 - (1.0 - s) * (1.0 - s)),
            K::ReversePortal => self.size * (1.0 - (self.age as f32 + t) / (life * 1.5)),
            K::Shriek => {
                return (self.size * (s * 0.75).clamp(0.0, 1.0), 1.0 - s.clamp(0.0, 1.0));
            }
            K::VaultConnection => {
                return (self.size, 0.6 * ((s - 0.25) / 0.75).clamp(0.0, 1.0));
            }
            K::Flash => {
                let k = self.age as f32 + t - 1.0;
                return (7.1 * (k * std::f32::consts::PI / 4.0).sin(), 0.6 - k * 0.125);
            }
            _ => self.size,
        };
        (size, self.rgba[3])
    }

    /// Packed light (block × 16, sky × 16 << 8).
    fn light(&self, w: &TickWorld<'_>, t: f32) -> u32 {
        let full = 0xf0 | (0xf0 << 8);
        let b = self.pos.map(|v| v.floor() as i32);
        let (block, sky) = w.light(b[0], b[1], b[2]);
        let (block, sky) = (u32::from(block) * 16, u32::from(sky) * 16);
        let life = self.life.max(1) as f32;
        let emissive = |f: f32| (block + (f.clamp(0.0, 1.0) * 240.0) as u32).min(240) | (sky << 8);
        match self.kind {
            k if k.flame() => emissive((self.age as f32 + t) / life),
            K::Glow | K::ElectricSpark | K::WaxOn | K::WaxOff | K::Scrape => emissive((self.age as f32 + t) / life),
            K::Portal | K::ReversePortal | K::Enchant | K::Nautilus => emissive((self.age as f32 / life).powi(4)),
            K::DrippingObsidianTear | K::FallingObsidianTear | K::LandingObsidianTear => 240 | (sky << 8),
            // Firefly: its own glow only, sky ignored.
            K::Firefly => ((255.0 * fade(progress(self.age as f32 + t, self.life), 0.1, 0.3)) as u32).min(240),
            K::Lava
            | K::SculkCharge
            | K::SculkChargePop
            | K::Shriek
            | K::Vibration
            | K::TrialDetection
            | K::TrialDetectionOminous
            | K::OminousSpawning
            | K::VaultConnection => 240 | (sky << 8),
            K::Explosion
            | K::SweepAttack
            | K::TotemOfUndying
            | K::EndRod
            | K::Firework
            | K::SonicBoom
            | K::Gust
            | K::SmallGust
            | K::SquidInk
            | K::GlowSquidInk => full,
            K::SculkSoul => 240 | (sky << 8),
            _ => block | (sky << 8),
        }
    }
}

/// Factory (§ 3): `p` = velocity-parameter passed.
#[allow(clippy::needless_range_loop)]
fn create(kind: Kind, o: &Options, pos: [f64; 3], p: [f64; 3], rng: Rng, spell: &mut Rng, m: &Make<'_>) -> Option<P> {
    let a = m.a;
    let pos = match kind {
        K::AngryVillager => [pos[0], pos[1] + 0.5, pos[2]],
        // Suspended particles: created 1/8 of a block lower.
        K::Underwater | K::SporeBlossomAir | K::CrimsonSpore | K::WarpedSpore => [pos[0], pos[1] - 0.125, pos[2]],
        _ => pos,
    };
    let mut q = P::base(kind, pos, rng);
    let life_8 = |q: &mut P| (8.0 / (q.r() * 0.8 + 0.2)) as i32;
    match kind {
        K::Block | K::BlockCrumble | K::DustPillar => {
            let b = *a.block(o.state)?;
            if b.none {
                return None;
            }
            q.with_vel(p);
            q.gravity = 1.0;
            Make::tint(&mut q, m.terrain.get(o.state as usize), 0.6);
            q.size /= 2.0;
            q.chunk_uv();
            q.block_tex = b.tex + 1;
            if kind == K::BlockCrumble {
                q.vel = [0.0; 3];
                q.life = q.rng.int(10) + 1;
            } else if kind == K::DustPillar {
                let g = [q.rng.gauss(), q.rng.gauss(), q.rng.gauss()];
                q.vel = [g[0] / 30.0, p[1] + g[1] / 2.0, g[2] / 30.0];
                q.life = 20 + q.rng.int(20);
            }
            if o.power != 1.0 {
                q.power(f64::from(o.power));
            }
            if o.scale != 1.0 {
                q.scale(o.scale);
            }
        }
        K::Item | K::ItemSnowball | K::ItemSlime | K::ItemCobweb | K::SulfurCubeGoo => {
            q.sprite = match kind {
                // One layer of the model at random when there are several.
                K::Item => match usize::try_from(o.item).ok().and_then(|i| a.items.get(i)).map(Vec::as_slice) {
                    Some(&[s]) => s,
                    Some(l) if !l.is_empty() => l[q.rng.int(l.len() as u32) as usize],
                    _ => 0,
                },
                K::ItemSnowball => a.snowball,
                K::ItemSlime => a.slime,
                // Sulfur cube fragment: first sprite of its set (particle atlas).
                K::SulfurCubeGoo => a.set(kind).first().copied().unwrap_or(0),
                _ => a.cobweb,
            };
            q.with_vel([0.0; 3]);
            if kind == K::Item {
                q.vel = [q.vel[0] * 0.1 + p[0], q.vel[1] * 0.1 + p[1], q.vel[2] * 0.1 + p[2]];
            }
            q.gravity = 1.0;
            q.size /= 2.0;
            q.chunk_uv();
            q.translucent = a.translucent.get(q.sprite as usize).copied().unwrap_or(false);
        }
        K::Poof | K::Spit => {
            for i in 0..3 {
                q.vel[i] = p[i] + (q.rd() * 2.0 - 1.0) * 0.05;
            }
            q.gravity = if kind == K::Spit { 0.5 } else { -0.1 };
            q.friction = 0.9;
            let c = 0.7 + 0.3 * q.r();
            q.gray(c);
            q.size = 0.1 * (q.r() * q.r() * 6.0 + 1.0);
            q.life = (16.0 / (q.r() * 0.8 + 0.2)) as i32 + 2;
        }
        K::SonicBoom | K::Gust | K::SmallGust => {
            // Large explosion (sonic wave): 16 ticks, size 1.5; burst: 12 to 15 ticks,
            // size 1 (× 0.15 for the small one). Full light, frame by age.
            if kind == K::SonicBoom {
                q.life = 6 + q.rng.int(4);
                let c = q.r() * 0.6 + 0.4;
                q.gray(c);
                q.life = 16;
                q.size = 1.5;
            } else {
                q.life = 12 + q.rng.int(4);
                q.size = if kind == K::SmallGust { 0.15 } else { 1.0 };
            }
            q.collide = false;
        }
        K::SquidInk | K::GlowSquidInk => {
            q.friction = 0.92;
            q.size = 0.5;
            q.color(if kind == K::SquidInk { [0.0; 3] } else { [51.0 / 255.0, 204.0 / 255.0, 153.0 / 255.0] });
            q.life = (0.5 * 12.0 / (q.r() * 0.8 + 0.2)) as i32;
            q.collide = false;
            q.vel = p;
            q.translucent = true;
        }
        K::Soul | K::SculkSoul => {
            // "Rising" template (like the flame) but with collisions, × 1.5, translucent.
            q.with_vel(p);
            for i in 0..3 {
                q.vel[i] = q.vel[i] * f64::from(0.01f32) + p[i];
            }
            // The box is re-centred on its own centre (before the offset) by the scale.
            q.scale(1.5);
            // Offset applied to the position alone (the box does not follow: lost at the first
            // movement, like the game).
            for i in 0..3 {
                q.pos[i] += f64::from(q.r() - q.r()) * 0.05;
            }
            q.life = (8.0 / (q.r() * 0.8 + 0.2)) as i32 + 4;
            q.friction = 0.96;
            q.translucent = true;
        }
        K::Fishing => {
            // Wake: velocity passed as is, life counted down, sprite by 60 − life.
            q.with_vel([0.0; 3]);
            q.set_size(0.01, 0.01);
            q.life = (8.0 / (q.r() * 0.8 + 0.2)) as i32;
            q.vel = p;
        }
        K::PauseMobGrowth | K::ResetMobGrowth => {
            q.with_vel(p);
            q.vel = [p[0], p[1] + if kind == K::ResetMobGrowth { 0.03 } else { -0.03 }, p[2]];
            q.size *= q.r() * 0.6 + 0.5;
            q.life = 8;
        }
        K::GustEmitterLarge | K::GustEmitterSmall => return None,
        K::Smoke | K::LargeSmoke | K::Ash | K::WhiteAsh | K::DustPlume | K::NoxiousGas => {
            let (dir, cmax, life_max, g, collide, scale, param) = match kind {
                K::Smoke => ([0.1, 0.1, 0.1], 0.3, 8.0, -0.1, true, 1.0, p),
                K::LargeSmoke => ([0.1, 0.1, 0.1], 0.3, 8.0, -0.1, true, 2.5, p),
                K::DustPlume => ([0.7, 0.6, 0.7], 0.5, 7.0, 0.5, false, 1.0, [p[0], p[1] + f64::from(0.15f32), p[2]]),
                K::NoxiousGas => ([0.1, 0.1, 0.1], 0.3, 5.0, -0.02, true, 3.0, p),
                K::Ash => ([0.1, -0.1, 0.1], 0.5, 20.0, 0.1, false, 1.0, [0.0; 3]),
                _ => {
                    let mut r = || f64::from(q.rng.f());
                    let w = [-1.9 * r() * r() * 0.1, -0.5 * r() * r() * 0.5, -1.9 * r() * r() * 0.1];
                    ([0.1, -0.1, 0.1], 0.0, 20.0, 0.0125, false, 1.0, w)
                }
            };
            q.with_vel([0.0; 3]);
            for i in 0..3 {
                q.vel[i] = q.vel[i] * dir[i] + param[i];
            }
            let c = q.r() * cmax;
            q.gray(c);
            if kind == K::WhiteAsh {
                q.color(rgb_of(0xbab1c2));
            }
            q.size *= 0.75 * scale;
            q.life = ((life_max / (q.r() * 0.8 + 0.2)) * scale).max(1.0) as i32;
            q.friction = 0.96;
            q.gravity = g;
            q.ceil = true;
            q.collide = collide;
            match kind {
                K::DustPlume => {
                    // Mauve-grey (0xbab1c2) darkened by 0 to 0.2.
                    let shift = q.r() * 0.2;
                    let c = rgb_of(0xbab1c2);
                    q.color([c[0] - shift, c[1] - shift, c[2] - shift]);
                }
                K::NoxiousGas => {
                    q.gray(1.0);
                    q.life = (6.0 / (q.r() * 0.5 + 0.5) * 3.0) as i32;
                    q.translucent = true;
                }
                _ => {}
            }
        }
        K::Flame | K::SoulFireFlame | K::SmallFlame | K::CopperFireFlame => {
            q.with_vel(p);
            for i in 0..3 {
                q.vel[i] = q.vel[i] * 0.01 + p[i];
            }
            for i in 0..3 {
                q.pos[i] += f64::from(q.r() - q.r()) * 0.05;
            }
            q.life = (8.0 / (q.r() * 0.8 + 0.2)) as i32 + 4;
            q.friction = 0.96;
            q.collide = false;
            if kind == K::SmallFlame {
                // Box re-centred on itself: the position offset is lost at the first
                // movement, like the large flame.
                q.scale(0.5);
            }
        }
        K::Lava => {
            q.with_vel([0.0; 3]);
            q.vel = q.vel.map(|v| v * 0.8);
            q.vel[1] = f64::from(q.r()) * 0.4 + 0.05;
            q.gravity = 0.75;
            q.friction = 0.999;
            q.size *= 0.2 + q.r() * 2.0;
            q.life = (16.0 / (q.r() * 0.8 + 0.2)) as i32;
        }
        K::DrippingWater | K::DrippingLava | K::DrippingDripstoneWater | K::DrippingDripstoneLava => {
            q.set_size(0.01, 0.01);
            q.gravity = 0.06 * 0.02;
            q.life = 40;
            if matches!(kind, K::DrippingWater | K::DrippingDripstoneWater) {
                q.color([0.2, 0.3, 1.0]);
            }
        }
        K::FallingWater | K::FallingLava | K::LandingLava | K::FallingDripstoneWater | K::FallingDripstoneLava => {
            // The drip factories ignore the passed velocity: stationary start.
            q.set_size(0.01, 0.01);
            q.gravity = 0.06;
            q.life = if kind == K::LandingLava {
                (16.0 / (q.r() * 0.8 + 0.2)) as i32
            } else {
                (64.0 / (q.r() * 0.8 + 0.2)) as i32
            };
            q.color(if matches!(kind, K::FallingWater | K::FallingDripstoneWater) {
                [0.2, 0.3, 1.0]
            } else {
                [1.0, 0.2857, 0.0833]
            });
        }
        K::Rain | K::Splash => {
            q.with_vel([0.0; 3]);
            q.vel[0] *= 0.3;
            q.vel[2] *= 0.3;
            q.vel[1] = f64::from(q.r()) * 0.2 + 0.1;
            q.set_size(0.01, 0.01);
            q.gravity = if kind == K::Rain { 0.06 } else { 0.04 };
            q.life = life_8(&mut q);
            if kind == K::Splash && p[1] == 0.0 && (p[0] != 0.0 || p[2] != 0.0) {
                q.vel = [p[0], 0.1, p[2]];
            }
        }
        K::Bubble => {
            for i in 0..3 {
                q.vel[i] = p[i] * 0.2 + (q.rd() * 2.0 - 1.0) * 0.02;
            }
            q.set_size(0.02, 0.02);
            q.size *= q.r() * 0.6 + 0.2;
            q.life = life_8(&mut q);
        }
        K::BubblePop => {
            q.vel = p;
            q.life = 4;
            q.gravity = 0.2;
            q.friction = 1.0;
        }
        K::Crit | K::EnchantedHit | K::DamageIndicator => {
            let p = if kind == K::DamageIndicator { [p[0], p[1] + 1.0, p[2]] } else { p };
            q.with_vel([0.0; 3]);
            for i in 0..3 {
                q.vel[i] = q.vel[i] * 0.1 + p[i] * 0.4;
            }
            q.friction = 0.7;
            q.gravity = 0.5;
            let c = q.r() * 0.3 + 0.6;
            q.gray(c);
            q.size *= 0.75;
            q.life = ((6.0 / (q.r() * 0.8 + 0.6)) as i32).max(1);
            q.collide = false;
            match kind {
                K::EnchantedHit => {
                    q.rgba[0] *= 0.3;
                    q.rgba[1] *= 0.8;
                }
                K::DamageIndicator => q.life = 20,
                _ => {}
            }
        }
        K::Heart | K::AngryVillager => {
            q.with_vel([0.0; 3]);
            q.vel = q.vel.map(|v| v * 0.01);
            q.vel[1] += 0.1;
            q.friction = 0.86;
            q.ceil = true;
            q.size *= 1.5;
            q.life = 16;
            q.collide = false;
        }
        K::HappyVillager | K::Composter | K::EggCrack | K::Mycelium | K::Dolphin => {
            q.with_vel(p);
            q.vel = q.vel.map(|v| v * 0.02);
            q.set_size(0.02, 0.02);
            q.size *= q.r() * 0.6 + 0.5;
            let c = q.r() * 0.1 + 0.2;
            q.gray(c);
            q.life = (20.0 / (q.r() * 0.8 + 0.2)) as i32;
            q.collide = false;
            match kind {
                K::HappyVillager | K::EggCrack => q.gray(1.0),
                K::Composter => {
                    q.gray(1.0);
                    q.life = 3 + q.rng.int(5);
                }
                K::Dolphin => {
                    q.color([0.3, 0.5, 1.0]);
                    q.rgba[3] = 1.0 - q.r() * 0.7;
                    q.life /= 2;
                }
                _ => {}
            }
        }
        K::Note => {
            q.with_vel([0.0; 3]);
            q.vel = q.vel.map(|v| v * 0.01);
            q.vel[1] += 0.2;
            q.friction = 0.66;
            q.ceil = true;
            q.size *= 1.5;
            q.life = 6;
            let c = p[0] as f32;
            let tau = std::f32::consts::TAU;
            let ch = |k: f32| ((tau * (c + k)).sin() * 0.65 + 0.35).max(0.0);
            q.color([ch(0.0), ch(1.0 / 3.0), ch(2.0 / 3.0)]);
        }
        K::Portal => {
            q.param = p;
            q.size = 0.1 * (q.r() * 0.2 + 0.5);
            let c = q.r() * 0.6 + 0.4;
            q.color([0.9 * c, 0.3 * c, c]);
            q.life = 40 + (q.r() * 10.0) as i32;
            q.collide = false;
        }
        K::Enchant | K::Nautilus | K::VaultConnection => {
            q.param = p;
            q.set_pos([pos[0] + p[0], pos[1] + p[1], pos[2] + p[2]]);
            q.old = q.pos;
            q.size = 0.1 * (q.r() * 0.5 + 0.2);
            let c = q.r() * 0.6 + 0.4;
            q.color([0.9 * c, 0.9 * c, c]);
            q.collide = false;
            q.life = 30 + (q.r() * 10.0) as i32;
            if kind == K::VaultConnection {
                // Luminous, translucent (alpha from 0 to 0.6 between 25% and 100% of the life), ×1.5.
                q.scale(1.5);
                q.translucent = true;
            }
        }
        K::OminousSpawning => {
            q.param = p;
            q.set_pos([pos[0] + p[0], pos[1] + p[1], pos[2] + p[2]]);
            q.old = q.pos;
            q.size = 0.1 * (q.r() * 0.5 + 0.2);
            q.collide = false;
            q.life = 25 + (q.r() * 5.0) as i32;
            let e = 3.0 + q.r() * 2.0;
            q.size *= e;
            q.color([0x45 as f32 / 255.0, 0xae as f32 / 255.0, 1.0]);
        }
        K::WhiteSmoke => {
            q.with_vel([0.0; 3]);
            for i in 0..3 {
                q.vel[i] = q.vel[i] * 0.1 + p[i];
            }
            q.size *= 0.75;
            q.life = ((8.0 / (q.r() * 0.8 + 0.2)) as i32).max(1);
            q.friction = 0.96;
            q.gravity = -0.1;
            q.ceil = true;
            q.color([0.729_411_8, 0.694_117_67, 0.760_784_3]);
        }
        K::DragonBreath => {
            q.friction = 0.96;
            q.vel = p;
            let (cr, cb) = (q.r(), q.r());
            q.color([0.7176471 + cr * 0.157_647_1, 0.0, 0.8235294 + cb * 0.152_941_2]);
            q.size *= 0.75;
            q.life = (20.0 / (q.r() * 0.8 + 0.2)) as i32;
            q.collide = false;
            if o.power != 1.0 {
                q.power(f64::from(o.power));
            }
        }
        K::SculkCharge | K::SculkChargePop => {
            // Imposed velocity, no collision, luminous, translucent; `scale` = roll (charge).
            q.vel = p;
            q.friction = 0.96;
            q.collide = false;
            q.translucent = true;
            if kind == K::SculkCharge {
                q.scale(1.5);
                q.roll = o.scale;
                q.old_roll = o.scale;
                q.life = q.rng.int(12) + 8;
            } else {
                q.scale(1.0);
                q.life = q.rng.int(4) + 6;
            }
        }
        K::Vibration => {
            // Vibration signal: half-side 0.3, opaque, duration = trip; target in `start`
            // (block: its centre; entity: kept up to date by `track_vibrations`).
            let (dest, ticks) = o.vib?;
            q.vel = [0.0; 3];
            q.gravity = 0.0;
            q.collide = false;
            q.size = 0.3;
            q.life = ticks;
            q.rgba[3] = 1.0;
            q.translucent = true;
            q.sprite = a.set(kind).first().copied().unwrap_or(0);
            match dest {
                rc_proto_v777::entity::VibDest::Block(b) => {
                    q.start = [f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5];
                    q.face_target();
                    q.old_roll = q.roll;
                    q.param[1] = q.param[0];
                }
                rc_proto_v777::entity::VibDest::Entity { id, y_offset } => {
                    q.start = pos;
                    q.vib_target = Some((id, y_offset));
                }
            }
        }
        K::Shriek => {
            // `state` = delay in ticks; slow rise of 0.1 per tick over 30 ticks.
            q.size = 0.85;
            q.life = 30;
            q.param[0] = f64::from(o.state);
            q.vel = [0.0, 0.1, 0.0];
            q.translucent = true;
        }
        K::TrialDetection | K::TrialDetectionOminous => {
            q.with_vel([0.0; 3]);
            q.vel = [p[0], q.vel[1] * 0.9 + p[1], p[2]];
            q.size *= 0.75 * 1.5;
            let r = 0.5 + q.r() * 0.5;
            q.life = ((8.0 / r * 1.5) as i32).max(1);
            q.friction = 0.96;
            q.gravity = -0.1;
            q.ceil = true;
        }
        K::TrialOmen | K::RaidOmen | K::Infested => {
            let (a1, a2) = (spell.d(), spell.d());
            q.with_vel([0.5 - a1, p[1], 0.5 - a2]);
            q.vel[1] *= 0.2;
            if p[0] == 0.0 && p[2] == 0.0 {
                q.vel[0] *= 0.1;
                q.vel[2] *= 0.1;
            }
            q.friction = 0.96;
            q.gravity = -0.1;
            q.ceil = true;
            q.size *= 0.75;
            q.life = life_8(&mut q);
            q.collide = false;
            q.translucent = true;
            q.alpha0 = 1.0;
        }
        K::Explosion | K::SweepAttack => {
            q.life = if kind == K::Explosion { 6 + q.rng.int(4) } else { 4 };
            let c = q.r() * 0.6 + 0.4;
            q.gray(c);
            let f = (1.0 - p[0] * 0.5) as f32;
            q.size = if kind == K::Explosion { 2.0 * f } else { f };
        }
        K::Cloud | K::Sneeze => {
            q.with_vel([0.0; 3]);
            for i in 0..3 {
                q.vel[i] = q.vel[i] * 0.1 + p[i];
            }
            q.friction = 0.96;
            let c = 1.0 - q.r() * 0.3;
            q.gray(c);
            q.size *= 1.875;
            q.life = (((8.0 / (q.r() * 0.8 + 0.3)) as i32) as f32 * 2.5).max(1.0) as i32;
            q.collide = false;
            q.translucent = true;
            if kind == K::Sneeze {
                q.color([0.22, 1.0, 0.53]);
                q.rgba[3] = 0.4;
            }
        }
        K::Dust => {
            q.with_vel(p);
            q.vel = q.vel.map(|v| v * 0.1);
            q.friction = 0.96;
            q.ceil = true;
            let scale = o.scale.clamp(0.01, 4.0);
            q.size *= 0.75 * scale;
            q.life = ((((8.0 / (q.r() * 0.8 + 0.2)) as i32) as f32 * scale) as i32).max(1);
            let f = q.r() * 0.4 + 0.6;
            for i in 0..3 {
                q.rgba[i] = (q.r() * 0.2 + 0.8) * o.rgb[i] * f;
            }
        }
        K::EntityEffect | K::Effect | K::InstantEffect | K::Witch => {
            let (a1, a2) = (spell.d(), spell.d());
            q.with_vel([0.5 - a1, p[1], 0.5 - a2]);
            q.vel[1] *= 0.2;
            if p[0] == 0.0 && p[2] == 0.0 {
                q.vel[0] *= 0.1;
                q.vel[2] *= 0.1;
            }
            q.friction = 0.96;
            q.gravity = -0.1;
            q.ceil = true;
            q.size *= 0.75;
            q.life = life_8(&mut q);
            q.collide = false;
            q.translucent = true;
            match kind {
                K::EntityEffect => {
                    q.color(rgb_of(o.argb));
                    q.rgba[3] = (o.argb >> 24) as f32 / 255.0;
                }
                K::Effect | K::InstantEffect => {
                    q.color(o.rgb);
                    q.power(f64::from(o.power));
                }
                _ => {
                    let c = q.r() * 0.5 + 0.35;
                    q.color([c, 0.0, c]);
                }
            }
            q.alpha0 = q.rgba[3];
        }
        K::CampfireCosySmoke | K::CampfireSignalSmoke => {
            q.size *= 3.0;
            q.set_size(0.25, 0.25);
            let signal = kind == K::CampfireSignalSmoke;
            q.life = if signal { 280 } else { 80 } + q.rng.int(50);
            q.gravity = 3.0e-6;
            q.vel = [p[0], p[1] + f64::from(q.r()) / 500.0, p[2]];
            q.rgba[3] = if signal { 0.95 } else { 0.9 };
            q.translucent = true;
        }
        K::FallingDust => {
            let b = *a.block(o.state)?;
            if b.invisible {
                return None;
            }
            Make::tint(&mut q, m.dust.get(o.state as usize), 1.0);
            q.size *= 0.675;
            q.life = ((((32.0 / (q.r() * 0.8 + 0.2)) as i32) as f32 * 0.9) as i32).max(1);
            q.roll = q.r() * std::f32::consts::TAU;
            q.old_roll = q.roll;
            q.roll_speed = (q.r() - 0.5) * 0.1;
        }
        K::Snowflake => {
            for i in 0..3 {
                q.vel[i] = p[i] + (q.rd() * 2.0 - 1.0) * 0.05;
            }
            q.gravity = 0.225;
            q.friction = 1.0;
            q.size = 0.1 * (q.r() * q.r() + 1.0);
            q.life = (16.0 / (q.r() * 0.8 + 0.2)) as i32 + 2;
            q.color([0.923, 0.964, 0.999]);
        }
        K::Glow | K::ElectricSpark | K::WaxOn | K::WaxOff | K::Scrape => {
            if kind == K::Glow {
                let (a1, a2) = (q.rd(), q.rd());
                q.with_vel([0.5 - a1, p[1], 0.5 - a2]);
                q.vel[1] *= 0.2;
                if p[0] == 0.0 && p[2] == 0.0 {
                    q.vel[0] *= 0.1;
                    q.vel[2] *= 0.1;
                }
                let c = if q.r() < 0.5 { [0.6, 1.0, 0.8] } else { [0.08, 0.4, 0.4] };
                q.color(c);
                q.life = life_8(&mut q);
            } else {
                q.with_vel([0.0; 3]);
                match kind {
                    K::ElectricSpark => {
                        q.vel = p.map(|v| v * 0.25);
                        q.life = 2 + q.rng.int(2);
                        q.color([1.0, 0.9, 1.0]);
                    }
                    K::WaxOn | K::WaxOff => {
                        q.vel = [p[0] * 0.005, p[1] * 0.01, p[2] * 0.005];
                        q.life = 10 + q.rng.int(30);
                        q.color(if kind == K::WaxOn { [0.91, 0.55, 0.08] } else { [1.0, 0.9, 1.0] });
                    }
                    _ => {
                        q.vel = p.map(|v| v * 0.01);
                        q.life = 10 + q.rng.int(30);
                        let c = if q.r() < 0.5 { [0.29, 0.58, 0.51] } else { [0.43, 0.77, 0.62] };
                        q.color(c);
                    }
                }
            }
            q.friction = 0.96;
            q.ceil = true;
            q.size *= 0.75;
            q.collide = false;
        }
        K::TotemOfUndying | K::EndRod => {
            q.vel = p;
            q.size *= 0.75;
            q.life = 60 + q.rng.int(12);
            q.translucent = true;
            if kind == K::TotemOfUndying {
                q.gravity = 1.25;
                q.friction = 0.6;
                let c = if q.rng.int(4) == 0 {
                    [0.6 + q.r() * 0.2, 0.6 + q.r() * 0.3, q.r() * 0.2]
                } else {
                    [0.1 + q.r() * 0.2, 0.4 + q.r() * 0.3, q.r() * 0.2]
                };
                q.color(c);
            } else {
                q.gravity = 0.0125;
                q.friction = 0.91;
                q.collide = false;
                q.fade = Some(rgb_of(0xf2dec9));
            }
        }
        K::Flash => {
            q.life = 4;
            q.translucent = true;
            q.color(rgb_of(o.argb));
        }
        K::Firework => {
            // Spark ("simple animated" template): imposed velocity, translucent (alpha 0.99).
            q.gravity = 0.1;
            q.friction = 0.91;
            q.vel = p;
            q.size *= 0.75;
            q.life = 48 + q.rng.int(12);
            q.rgba[3] = 0.99;
            q.translucent = true;
        }
        K::BlockMarker => {
            // "Particle" sprite of the block, whole; stationary, 80 ticks, fixed size 0.5. Blocks
            // without faces (barrier, light): item texture copied into the particle atlas.
            match a.markers.get(o.state as usize).copied().filter(|&m| m != 0) {
                Some(m) => {
                    q.sprite = m;
                    q.translucent = a.translucent.get(m as usize).copied().unwrap_or(false);
                }
                None => {
                    let b = *a.block(o.state)?;
                    if b.none {
                        return None;
                    }
                    q.block_tex = b.tex + 1;
                }
            }
            q.life = 80;
            q.collide = false;
            q.size = 0.5;
        }
        K::Underwater | K::SporeBlossomAir | K::CrimsonSpore | K::WarpedSpore => {
            match kind {
                K::Underwater => {}
                K::CrimsonSpore => {
                    let g = [q.rng.gauss(), q.rng.gauss(), q.rng.gauss()];
                    q.with_vel([g[0] * f64::from(1.0e-6f32), g[1] * f64::from(1.0e-4f32), g[2] * f64::from(1.0e-6f32)]);
                }
                K::SporeBlossomAir => q.with_vel([0.0, f64::from(-0.8f32), 0.0]),
                _ => {
                    let (r1, r2) = (q.r(), q.r());
                    q.with_vel([0.0, f64::from(r1) * -1.9 * f64::from(r2) * 0.1, 0.0]);
                }
            }
            q.set_size(0.01, 0.01);
            // Size × (0.2..0.8) without initial velocity, × (0.6..1.2) with.
            q.size *= q.r() * 0.6 + if kind == K::Underwater { 0.2 } else { 0.6 };
            q.life = (16.0 / (q.r() * 0.8 + 0.2)) as i32;
            q.collide = false;
            q.friction = 1.0;
            match kind {
                K::Underwater => q.color([0.4, 0.4, 0.7]),
                K::CrimsonSpore => q.color([0.9, 0.4, 0.5]),
                K::SporeBlossomAir => {
                    q.life = 500 + q.rng.int(501);
                    q.gravity = 0.01;
                    q.color([0.32, 0.5, 0.22]);
                }
                _ => {
                    q.color([0.1, 0.1, 0.3]);
                    q.set_size(0.001, 0.001);
                }
            }
        }
        K::DrippingHoney | K::DrippingObsidianTear => {
            q.set_size(0.01, 0.01);
            q.gravity = 0.06 * 0.02 * 0.01;
            q.life = 100;
            q.color(if kind == K::DrippingHoney { [0.622, 0.508, 0.082] } else { TEAR });
        }
        K::FallingHoney | K::FallingObsidianTear => {
            q.set_size(0.01, 0.01);
            q.gravity = 0.01;
            q.life = (64.0 / (q.r() * 0.8 + 0.2)) as i32;
            q.color(if kind == K::FallingHoney { [0.582, 0.448, 0.082] } else { TEAR });
        }
        K::LandingHoney | K::LandingObsidianTear => {
            q.set_size(0.01, 0.01);
            q.gravity = 0.06;
            let (n, c) = if kind == K::LandingHoney { (128.0, [0.522, 0.408, 0.082]) } else { (28.0, TEAR) };
            q.life = (n / (q.r() * 0.8 + 0.2)) as i32;
            q.color(c);
        }
        K::FallingNectar | K::FallingSporeBlossom => {
            q.set_size(0.01, 0.01);
            if kind == K::FallingNectar {
                q.life = (16.0 / (q.r() * 0.8 + 0.2)) as i32;
                q.gravity = 0.007;
                q.color([0.92, 0.782, 0.72]);
            } else {
                q.life = (64.0 / (0.1 + q.r() * 0.8)) as i32;
                q.gravity = 0.005;
                q.color([0.32, 0.5, 0.22]);
            }
        }
        K::ReversePortal => {
            q.vel = p;
            q.size = 0.1 * (q.r() * 0.2 + 0.5) * 1.5;
            let c = q.r() * 0.6 + 0.4;
            q.color([0.9 * c, 0.3 * c, c]);
            q.life = (q.r() * 2.0) as i32 + 60;
        }
        K::BubbleColumnUp => {
            q.gravity = -0.125;
            q.friction = 0.85;
            q.set_size(0.02, 0.02);
            q.size *= q.r() * 0.6 + 0.2;
            for i in 0..3 {
                q.vel[i] = p[i] * f64::from(0.2f32) + f64::from((q.r() * 2.0 - 1.0) * 0.02);
            }
            q.life = (40.0 / (q.r() * 0.8 + 0.2)) as i32;
        }
        K::CurrentDown => {
            q.life = (q.r() * 60.0) as i32 + 30;
            q.collide = false;
            q.vel = [0.0, -0.05, 0.0];
            q.set_size(0.02, 0.02);
            q.size *= q.r() * 0.6 + 0.2;
            q.gravity = 0.002;
        }
        K::CherryLeaves
        | K::PaleOakLeaves
        | K::RedPoplarLeaves
        | K::OrangePoplarLeaves
        | K::YellowPoplarLeaves
        | K::TintedLeaves => {
            // (fall, wind, swirl, offset, scale, start velocity)
            let (fall, wind, swirl, flow, scale, v0) = if kind == K::CherryLeaves {
                (0.25f32, 2.0f32, false, true, 1.0f32, 0.0f32)
            } else {
                (0.07, 10.0, true, false, 2.0, 0.021)
            };
            q.roll_speed = if q.rng.int(2) == 0 { -30f32 } else { 30.0 }.to_radians();
            let spin = if q.rng.int(2) == 0 { -5f32 } else { 5.0 }.to_radians();
            q.life = 300;
            q.gravity = fall * 1.2 * 0.0025;
            let s = scale * if q.rng.int(2) == 0 { 0.05 } else { 0.075 };
            q.size = s;
            q.set_size(f64::from(s), f64::from(s));
            q.friction = 1.0;
            q.vel = [0.0, -f64::from(v0), 0.0];
            let r = q.r();
            let ang = f64::from((r * 60.0).to_radians());
            q.param = [
                ang.cos() * f64::from(wind),
                ang.sin() * f64::from(wind),
                f64::from((1000.0 + r * 3000.0).to_radians()),
            ];
            q.start = [f64::from(wind), f64::from(u8::from(swirl) | u8::from(flow) << 1), f64::from(spin)];
            if kind == K::TintedLeaves {
                q.color(rgb_of(o.argb));
            }
        }
        K::Firefly => {
            let (a1, a2) = (q.rd(), q.rd());
            let up = if q.rng.int(2) == 0 { p[1] } else { -p[1] };
            q.with_vel([0.5 - a1, up, 0.5 - a2]);
            q.ceil = true;
            q.friction = 0.96;
            q.size *= 0.75;
            q.vel = q.vel.map(|v| v * f64::from(0.8f32));
            q.life = 200 + q.rng.int(101);
            q.scale(1.5);
            q.rgba[3] = 0.0;
            q.translucent = true;
        }
        K::DustColorTransition => {
            q.with_vel(p);
            q.vel = q.vel.map(|v| v * f64::from(0.1f32));
            q.friction = 0.96;
            q.ceil = true;
            let scale = o.scale.clamp(0.01, 4.0);
            q.size *= 0.75 * scale;
            q.life = ((((8.0 / (q.r() * 0.8 + 0.2)) as i32) as f32 * scale) as i32).max(1);
            let f = q.r() * 0.4 + 0.6;
            let mut c = [0f32; 6];
            for (i, v) in c.iter_mut().enumerate() {
                let base = if i < 3 { o.rgb[i] } else { o.rgb2[i - 3] };
                *v = (q.r() * 0.2 + 0.8) * base * f;
            }
            q.color([c[0], c[1], c[2]]);
            // End colour kept in `param` (blended at display).
            q.param = [f64::from(c[3]), f64::from(c[4]), f64::from(c[5])];
        }
        K::SulfurBubbles => {
            q.gravity = -0.04;
            q.friction = 0.85;
            q.set_size(0.02, 0.02);
            // The z parameter comes from the transmitted y component (like the game).
            q.vel[0] = p[0] * f64::from(0.2f32) + f64::from((q.r() * 2.0 - 1.0) * 0.02);
            q.vel[2] = p[1] * f64::from(0.2f32) + f64::from((q.r() * 2.0 - 1.0) * 0.02);
            let s0 = 0.02 + 0.02 * q.r();
            q.size = s0;
            q.life = i32::MAX;
            q.start = pos;
            q.param = [pos[1] + 3.0, pos[1], f64::from(s0)];
        }
        K::ExplosionEmitter | K::NoxiousGasCloud => return None,
    }
    // Spells near the player aiming with a spyglass: alpha (and original alpha) at 0; the entity
    // effect then restores the alpha of its options.
    if kind.spell() && kind != K::EntityEffect && near_scope(m.scope, pos) {
        q.rgba[3] = 0.0;
    }
    if kind.animated() {
        q.pick_age(a);
    } else if !matches!(kind, K::Item | K::ItemSnowball | K::ItemSlime | K::ItemCobweb | K::SulfurCubeGoo) {
        q.pick_random(a);
    }
    q.alpha0 = q.rgba[3];
    Some(q)
}

/// Emitter without rendering (`explosion_emitter`).
struct Emitter {
    kind: Kind,
    pos: [f64; 3],
    age: i32,
    life: i32,
    rng: Rng,
}

/// Sound requested by the engine (drips, fireworks), played by the caller.
pub struct PSound {
    pub name: &'static str,
    pub pos: [f64; 3],
    pub category: rc_audio::Category,
    pub volume: f32,
    pub pitch: f32,
    /// Delayed by distance (1/40 s per block beyond 10 blocks).
    pub delayed: bool,
}

impl PSound {
    fn at(name: &'static str, pos: [f64; 3], category: rc_audio::Category, volume: f32, pitch: f32) -> Self {
        PSound { name, pos, category, volume, pitch, delayed: false }
    }

    /// Plays the sound; `camera` is used for the delay by distance.
    pub fn play(&self, audio: &mut rc_audio::AudioEngine, camera: [f64; 3]) {
        let d2: f64 = (0..3).map(|i| (self.pos[i] - camera[i]).powi(2)).sum();
        let mut req =
            rc_audio::SoundRequest::new(self.name, self.category).volume(self.volume).pitch(self.pitch).at(self.pos);
        if self.delayed && d2 > 100.0 {
            req = req.delay((d2.sqrt() / 40.0 * 20.0) as u32);
        }
        audio.play_request(req);
    }
}

/// Explosion of a firework (item component): shape 0 small ball, 1 large ball,
/// 2 star, 3 creeper head, 4 burst; colours and fade colours (RGB).
#[derive(Clone, Debug, Default)]
pub struct FireworkExplosion {
    pub shape: u8,
    pub colors: Vec<u32>,
    pub fades: Vec<u32>,
    pub trail: bool,
    pub twinkle: bool,
}

/// Firework trigger (without rendering): one explosion every two ticks.
struct Starter {
    pos: [f64; 3],
    vel: [f64; 3],
    explosions: Vec<FireworkExplosion>,
    life: i32,
    lifetime: i32,
    twinkle_delay: bool,
    sound: bool,
    rng: Rng,
}

/// Outlines (right half, x ≥ 0) of the star and the creeper head, in velocity units.
const STAR: [[f64; 2]; 6] = [
    [0.0, 1.0],
    [0.3455, 0.309],
    [0.9511, 0.309],
    [0.379_591_836_734_693_9, -0.126_530_612_244_897_95],
    [0.612_244_897_959_183_7, -0.804_081_632_653_061_2],
    [0.0, -0.359_183_673_469_387_73],
];
const CREEPER: [[f64; 2]; 12] = [
    [0.0, 0.2],
    [0.2, 0.2],
    [0.2, 0.6],
    [0.6, 0.6],
    [0.6, 0.2],
    [0.2, 0.2],
    [0.2, 0.0],
    [0.4, 0.0],
    [0.4, -0.6],
    [0.2, -0.6],
    [0.2, -0.4],
    [0.0, -0.4],
];

/// Velocities of a ball: shell of the cube `[−step, step]³` (the interior is skipped in z),
/// direction noised by ±0.5, norm `speed` within 5%.
fn ball(r: &mut Rng, speed: f64, steps: i32, out: &mut Vec<[f64; 3]>) {
    for y in -steps..=steps {
        for x in -steps..=steps {
            let mut z = -steps;
            while z <= steps {
                let xa = f64::from(x) + (r.d() - r.d()) * 0.5;
                let ya = f64::from(y) + (r.d() - r.d()) * 0.5;
                let za = f64::from(z) + (r.d() - r.d()) * 0.5;
                let len = (xa * xa + ya * ya + za * za).sqrt() / speed + r.gauss() * 0.05;
                out.push([xa / len, ya / len, za / len]);
                if y != -steps && y != steps && x != -steps && x != steps {
                    z += steps * 2 - 1;
                }
                z += 1;
            }
        }
    }
}

/// Velocities of a shape (star, creeper): first point, then three rotations around the
/// vertical, four sub-steps per segment, mirrored.
fn shape(r: &mut Rng, speed: f64, coords: &[[f64; 2]], flat: bool, out: &mut Vec<[f64; 3]>) {
    let (sx, sy) = (coords[0][0], coords[0][1]);
    out.push([sx * speed, sy * speed, 0.0]);
    let base = r.f() * std::f32::consts::PI;
    let k = if flat { 0.034 } else { 0.34 };
    for step in 0..3 {
        let angle = f64::from(base) + f64::from(step as f32 * std::f32::consts::PI) * k;
        let (mut ox, mut oy) = (sx, sy);
        for c in &coords[1..] {
            let (tx, ty) = (c[0], c[1]);
            let mut sub = 0.25;
            while sub <= 1.0 {
                let xa = (ox + (tx - ox) * sub) * speed;
                let ya = (oy + (ty - oy) * sub) * speed;
                let za = xa * angle.sin();
                let xa = xa * angle.cos();
                for flip in [-1.0, 1.0] {
                    out.push([xa * flip, ya, za * flip]);
                }
                sub += 0.25;
            }
            (ox, oy) = (tx, ty);
        }
    }
}

/// Firework colour of black dye (explosion without colour).
const FIREWORK_BLACK: u32 = 0x1e_1b1b;

// ---------- Engine ----------

/// Engine measurements (per-second summary).
#[derive(Default)]
struct Stats {
    tick_s: f64,
    ticks: u32,
    prepare_s: f64,
    frames: u32,
}

pub struct Particles {
    assets: ParticleAssets,
    quads: Vec<P>,
    /// A vibration signal may follow an entity (`track_vibrations`).
    vib_tracking: bool,
    /// Common clock of the animated sprites (one step per tick): like the game's atlas, all the
    /// particles of the same strip rise to the same frame at the same tick.
    anim_clock: u32,
    pending: Vec<P>,
    emitters: Vec<Emitter>,
    rng: Rng,
    /// "Particles" setting: 0 all, 1 reduced, 2 minimal.
    pub level: u8,
    /// Shared generator of the spells (§ 3, "spell" template).
    spell: Rng,
    camera: [f64; 3],
    /// Type of each network id.
    by_id: Vec<Option<Kind>>,
    children: Vec<Child>,
    /// Live (and waiting) of the capped category (`spore_blossom_air`).
    limited: usize,
    /// Sounds requested by the particles this tick (taken back by `take_sounds`).
    sounds: Vec<PSound>,
    /// Firework triggers in progress.
    starters: Vec<Starter>,
    /// Demonstration (`RUSTCRAFT_PARTICLES_DEMO`): types emitted, ticks elapsed.
    demo: Option<Vec<Kind>>,
    demo_ticks: u32,
    /// Measurement load (`RUSTCRAFT_PARTICLES_BENCH=n`).
    bench: usize,
    /// `RUSTCRAFT_PARTICLES_PAR`: 0 sequential, 1 pool, absent: depending on the threshold.
    parallel: Option<bool>,
    stats: Stats,
    report: bool,
    started: Instant,
    /// Per state: tint of the block fragments, colour of the falling dust (client data).
    terrain: Vec<StateTint>,
    dust: Vec<StateTint>,
    /// Eyes of the player aiming with a spyglass, read at the last tick.
    scope: Option<[f64; 3]>,
    /// Biome colours (palette, blend radius, biome of absent columns) for the
    /// tints resolved on entering the queue.
    biomes: (std::sync::Arc<rc_mesh::blocks::BiomePalette>, i32, u32),
    gas: GasTables,
}

impl Particles {
    /// Biome colours in force (palette of the received biomes, blend setting, plains).
    pub fn set_biomes(&mut self, palette: &std::sync::Arc<rc_mesh::blocks::BiomePalette>, blend: i32, plains: u32) {
        if !std::sync::Arc::ptr_eq(&self.biomes.0, palette) {
            self.biomes.0 = palette.clone();
        }
        (self.biomes.1, self.biomes.2) = (blend.clamp(0, 7), plains);
    }
    pub fn new(assets: ParticleAssets, data: &rc_data::VersionData) -> Self {
        let by_id = data
            .registry("minecraft:particle_type")
            .map(|r| r.entries.iter().map(|n| Kind::from_name(n)).collect())
            .unwrap_or_default();
        let demo = rc_data::env_str!("RUSTCRAFT_PARTICLES_DEMO").map(|v| {
            let list: Vec<Kind> = if v == "1" || v.is_empty() {
                DEMO.iter().map(|d| d.0).collect()
            } else {
                v.split(',').filter_map(Kind::from_name).collect()
            };
            eprintln!(
                "particle demo, rows of 10 from left to right and top to bottom: {}",
                list.iter().map(|k| k.name()).collect::<Vec<_>>().join(" ")
            );
            list
        });
        let bench = rc_data::env_str!("RUSTCRAFT_PARTICLES_BENCH").and_then(|v| v.parse().ok()).unwrap_or(0);
        let parallel = rc_data::env_str!("RUSTCRAFT_PARTICLES_PAR").map(|v| v != "0");
        let seed =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Particles {
            assets,
            quads: Vec::with_capacity(1024),
            vib_tracking: false,
            anim_clock: 0,
            pending: Vec::new(),
            emitters: Vec::new(),
            rng: Rng(seed),
            level: 0,
            spell: Rng(seed ^ 0x5eed),
            camera: [0.0; 3],
            by_id,
            children: Vec::new(),
            limited: 0,
            sounds: Vec::new(),
            starters: Vec::new(),
            report: demo.is_some() || bench > 0 || rc_data::env_flag!("RUSTCRAFT_PARTICLES_STATS"),
            demo,
            demo_ticks: 0,
            bench,
            parallel,
            stats: Stats::default(),
            started: Instant::now(),
            terrain: data.client_states.iter().map(|c| c.particle_tint).collect(),
            dust: data.client_states.iter().map(|c| c.dust).collect(),
            scope: None,
            biomes: (std::sync::Arc::new(Vec::new()), rc_mesh::blocks::BIOME_BLEND, 0),
            gas: GasTables::new(data),
        }
    }

    #[allow(dead_code)] // for the level_particles packets (wired elsewhere)
    /// Type of a network id (`minecraft:particle_type`), if supported.
    pub fn kind_of(&self, id: i32) -> Option<Kind> {
        usize::try_from(id).ok().and_then(|i| self.by_id.get(i)).copied().flatten()
    }

    /// Sounds requested by the particles since the last call.
    pub fn take_sounds(&mut self) -> Vec<PSound> {
        std::mem::take(&mut self.sounds)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.quads.len()
    }

    /// World change: everything is emptied.
    pub fn clear(&mut self) {
        self.quads.clear();
        self.pending.clear();
        self.emitters.clear();
        self.starters.clear();
        self.limited = 0;
    }

    /// Addition with the world filter (§ 1.2): distance to the camera unless forced type or call.
    /// Returns true if the particle was created.
    pub fn spawn(&mut self, kind: Kind, o: &Options, pos: [f64; 3], vel: [f64; 3]) -> bool {
        self.spawn_ex(kind, o, pos, vel, false)
    }

    /// "Always visible" addition (bubble columns, sulfur bubbles, marked packets): at
    /// minimal setting, 1 chance in 10 of being treated as reduced.
    pub fn spawn_visible(&mut self, kind: Kind, o: &Options, pos: [f64; 3], vel: [f64; 3]) -> bool {
        self.spawn_ex(kind, o, pos, vel, true)
    }

    /// World filter (§ 1.2) with or without "always visible".
    /// Vibration signals that follow an entity: target = feet + height; entity gone:
    /// signal removed. Nothing to do without a signal of this kind (sweep skipped).
    pub fn track_vibrations(&mut self, pos_of: impl Fn(i32) -> Option<[f64; 3]>) {
        if !self.vib_tracking {
            return;
        }
        let mut any = false;
        for p in self.quads.iter_mut().filter(|p| p.kind == K::Vibration && !p.removed) {
            let Some((id, dy)) = p.vib_target else { continue };
            any = true;
            match pos_of(id) {
                Some(e) => {
                    p.start = [e[0], e[1] + f64::from(dy), e[2]];
                    if p.age == 0 {
                        p.face_target();
                        p.old_roll = p.roll;
                        p.param[1] = p.param[0];
                    }
                }
                None => p.removed = true,
            }
        }
        self.vib_tracking = any;
    }

    pub fn spawn_ex(&mut self, kind: Kind, o: &Options, pos: [f64; 3], vel: [f64; 3], always: bool) -> bool {
        if kind == K::Vibration && matches!(o.vib, Some((rc_proto_v777::entity::VibDest::Entity { .. }, _))) {
            self.vib_tracking = true;
        }
        let forced = o.force || kind.forced();
        let d2: f64 = (0..3).map(|i| (pos[i] - self.camera[i]).powi(2)).sum();
        if !forced && d2 > MAX_DIST2 {
            return false;
        }
        // "Particles" setting (§ 1.2, step 3): minimal → nothing; reduced → 1 in 3 treated
        // as minimal.
        if !forced {
            let mut level = self.level;
            if always && level >= 2 && self.rng.int(10) == 0 {
                level = 1;
            }
            if level >= 2 || (level == 1 && self.rng.int(3) == 0) {
                return false;
            }
        }
        self.spawn_direct(kind, o, pos, vel)
    }

    /// Firework that explodes (entity event 17 of the rocket): without explosion, 2 to 4
    /// puffs; otherwise a trigger added directly to the engine. `sound`: rocket not
    /// silent.
    pub fn create_fireworks(&mut self, pos: [f64; 3], vel: [f64; 3], explosions: Vec<FireworkExplosion>, sound: bool) {
        if explosions.is_empty() {
            for _ in 0..self.rng.int(3) + 2 {
                let v = [self.rng.gauss() * 0.05, 0.005, self.rng.gauss() * 0.05];
                self.spawn(K::Poof, &Options::default(), pos, v);
            }
            return;
        }
        let mut lifetime = explosions.len() as i32 * 2 - 1;
        let twinkle_delay = explosions.iter().any(|e| e.twinkle);
        if twinkle_delay {
            lifetime += 15;
        }
        let rng = self.rng.fork();
        self.starters.push(Starter { pos, vel, explosions, life: 0, lifetime, twinkle_delay, sound, rng });
    }

    /// One tick of a trigger; returns false when it has finished.
    fn tick_starter(&mut self, s: &mut Starter) -> bool {
        let d2: f64 = (0..3).map(|i| (s.pos[i] - self.camera[i]).powi(2)).sum();
        let far = d2 >= 256.0;
        if s.life == 0 && s.sound {
            let large = s.explosions.len() >= 3 || s.explosions.iter().any(|e| e.shape == 1);
            let name = match (large, far) {
                (true, true) => "minecraft:entity.firework_rocket.large_blast_far",
                (true, false) => "minecraft:entity.firework_rocket.large_blast",
                (false, true) => "minecraft:entity.firework_rocket.blast_far",
                (false, false) => "minecraft:entity.firework_rocket.blast",
            };
            let pitch = 0.95 + s.rng.f() * 0.1;
            let mut snd = PSound::at(name, s.pos, rc_audio::Category::Ambient, 20.0, pitch);
            snd.delayed = true;
            self.sounds.push(snd);
        }
        if s.life % 2 == 0 && ((s.life / 2) as usize) < s.explosions.len() {
            let e = s.explosions[(s.life / 2) as usize].clone();
            let colors = if e.colors.is_empty() { vec![FIREWORK_BLACK] } else { e.colors.clone() };
            let mut flags = 0;
            if e.trail {
                flags |= SPARK_TRAIL;
            }
            if e.twinkle {
                flags |= SPARK_TWINKLE;
            }
            let mut out: Vec<[f64; 3]> = Vec::new();
            match e.shape {
                0 => ball(&mut s.rng, 0.25, 2, &mut out),
                1 => ball(&mut s.rng, 0.5, 4, &mut out),
                2 => shape(&mut s.rng, 0.5, &STAR, false, &mut out),
                3 => shape(&mut s.rng, 0.5, &CREEPER, true, &mut out),
                _ => {
                    let (ox, oz) = (s.rng.gauss() * 0.05, s.rng.gauss() * 0.05);
                    for _ in 0..70 {
                        let xa = s.vel[0] * 0.5 + s.rng.gauss() * 0.15 + ox;
                        let za = s.vel[2] * 0.5 + s.rng.gauss() * 0.15 + oz;
                        let ya = s.vel[1] * 0.5 + s.rng.d() * 0.5;
                        out.push([xa, ya, za]);
                    }
                }
            }
            for v in out {
                // The sparks go through the engine's factory (without world filter).
                let rng = self.rng.fork();
                let Some(mut q) =
                    create(K::Firework, &Options::default(), s.pos, v, rng, &mut self.spell, &make!(self))
                else {
                    continue;
                };
                q.param[0] = f64::from(flags);
                q.color(rgb_of(colors[s.rng.int(colors.len() as u32) as usize]));
                if !e.fades.is_empty() {
                    q.fade = Some(rgb_of(e.fades[s.rng.int(e.fades.len() as u32) as usize]));
                }
                self.pending.push(q);
            }
            let flash = Options { argb: colors[0], ..Options::default() };
            self.spawn_direct(K::Flash, &flash, s.pos, [0.0; 3]);
        }
        s.life += 1;
        if s.life > s.lifetime {
            if s.twinkle_delay && s.sound {
                let name = if far {
                    "minecraft:entity.firework_rocket.twinkle_far"
                } else {
                    "minecraft:entity.firework_rocket.twinkle"
                };
                let pitch = 0.9 + s.rng.f() * 0.15;
                let mut snd = PSound::at(name, s.pos, rc_audio::Category::Ambient, 20.0, pitch);
                snd.delayed = true;
                self.sounds.push(snd);
            }
            return false;
        }
        true
    }

    /// Direct addition to the engine, without filter (break and mining fragments).
    pub fn spawn_direct(&mut self, kind: Kind, o: &Options, pos: [f64; 3], vel: [f64; 3]) -> bool {
        let rng = self.rng.fork();
        if matches!(kind, K::ExplosionEmitter | K::GustEmitterLarge | K::GustEmitterSmall | K::NoxiousGasCloud) {
            let life = match kind {
                K::ExplosionEmitter => 8,
                K::GustEmitterLarge => 7,
                K::NoxiousGasCloud => 20,
                _ => 3,
            };
            self.emitters.push(Emitter { kind, pos, age: 0, life, rng });
            return true;
        }
        let Some(mut q) = create(kind, o, pos, vel, rng, &mut self.spell, &make!(self)) else { return false };
        // A tick executed in the constructor (§ 3, crit).
        if matches!(kind, K::Crit | K::EnchantedHit | K::DamageIndicator) {
            q.tick(&self.assets, None);
            if q.removed {
                return true;
            }
        }
        // Category cap: counted on entering the waiting queue.
        if kind.limited() {
            if self.limited >= SPORE_LIMIT {
                return true;
            }
            self.limited += 1;
        }
        self.pending.push(q);
        true
    }

    /// Engine tick (20 per second): quads, emitters, then waiting queue.
    pub fn tick(&mut self, w: &TickWorld<'_>, camera: [f64; 3]) {
        let t = Instant::now();
        self.camera = camera;
        self.scope = w.scope;
        self.anim_clock = self.anim_clock.wrapping_add(1);
        let (quads, assets) = (&mut self.quads, &self.assets);
        let par = self.parallel.unwrap_or(quads.len() >= PARALLEL_MIN);
        if par {
            quads.par_iter_mut().with_min_len(256).for_each(|p| p.tick(assets, Some(w)));
        } else {
            for p in quads.iter_mut() {
                p.tick(assets, Some(w));
            }
        }
        let children = &mut self.children;
        children.clear();
        let sounds = &mut self.sounds;
        let limited = &mut self.limited;
        let mut sparks: Vec<SparkSeed> = Vec::new();
        quads.retain_mut(|p| {
            if let Some(c) = p.child.take() {
                children.push(c);
            }
            if let Some((name, volume)) = p.sound.take() {
                sounds.push(PSound::at(name, p.pos, rc_audio::Category::Block, volume, 1.0));
            }
            if std::mem::take(&mut p.spark_child) {
                sparks.push((p.pos, [p.rgba[0], p.rgba[1], p.rgba[2]], p.fade, p.spark_flags() & SPARK_TWINKLE));
            }
            if p.removed && p.kind.limited() {
                *limited = limited.saturating_sub(1);
            }
            !p.removed
        });
        let mut children = std::mem::take(&mut self.children);
        for (k, pos, vel) in children.drain(..) {
            self.spawn(k, &Options::default(), pos, vel);
        }
        self.children = children;
        // Trail sparks: added directly, at half-life, same colour and fade.
        for (pos, rgb, fade, twinkle) in sparks {
            let rng = self.rng.fork();
            if let Some(mut q) =
                create(K::Firework, &Options::default(), pos, [0.0; 3], rng, &mut self.spell, &make!(self))
            {
                q.color(rgb);
                q.fade = fade;
                q.param[0] = f64::from(twinkle);
                q.age = q.life / 2;
                q.pick_age(&self.assets);
                self.pending.push(q);
            }
        }
        // Firework triggers (group without rendering).
        let mut starters = std::mem::take(&mut self.starters);
        starters.retain_mut(|s| self.tick_starter(s));
        starters.append(&mut self.starters);
        self.starters = starters;
        // Emitters: 6 explosions per tick for 8 ticks; bursts: 3 every 1 (large, 7
        // ticks, ±3) or 3 ticks (small, 3 ticks, ±1).
        let mut emitters = std::mem::take(&mut self.emitters);
        emitters.retain_mut(|e| {
            if e.kind == K::NoxiousGasCloud {
                // Gas cloud (20 ticks): one tick in two, a random point within 3 blocks at most
                // around the exit, 0.25 lower; "always visible" gas if it reaches there.
                let age = e.age;
                e.age += 1;
                if age >= e.life {
                    return false;
                }
                if e.age % 2 == 0 {
                    let src = e.pos.map(|v| v.floor() as i32);
                    let (dx, dz) = (f64::from(e.rng.f() - 0.5), f64::from(e.rng.f() - 0.5));
                    let n = (dx * dx + dz * dz).sqrt();
                    let (dx, dz) = if n < 1.0e-5 { (0.0, 0.0) } else { (dx / n, dz / n) };
                    let dist = f64::from(e.rng.f() * 3.0);
                    let c = src.map(|v| f64::from(v) + 0.5);
                    let at = [c[0] + dx * dist, c[1] - 0.25, c[2] + dz * dist];
                    if self.gas.reaches(w, src, at) {
                        self.spawn_visible(K::NoxiousGas, &Options::default(), at, [0.0; 3]);
                    }
                }
                return true;
            }
            if e.kind == K::ExplosionEmitter {
                for _ in 0..6 {
                    let d = [0, 1, 2].map(|i| e.pos[i] + (e.rng.d() - e.rng.d()) * 4.0);
                    self.spawn(K::Explosion, &Options::default(), d, [f64::from(e.age) / 8.0, 0.0, 0.0]);
                }
                e.age += 1;
                return e.age < e.life;
            }
            let (scale, gap) = if e.kind == K::GustEmitterLarge { (3.0, 0) } else { (1.0, 2) };
            if e.age % (gap + 1) == 0 {
                for _ in 0..3 {
                    let d = [0, 1, 2].map(|i| e.pos[i] + (e.rng.d() - e.rng.d()) * scale);
                    let v = [f64::from(e.age as f32 / e.life as f32), 0.0, 0.0];
                    self.spawn(K::Gust, &Options::default(), d, v);
                }
            }
            let age = e.age;
            e.age += 1;
            age != e.life
        });
        emitters.append(&mut self.emitters);
        self.emitters = emitters;
        // Waiting queue: progressive admission beyond 12288; biome tints resolved on
        // entry (same block and same kind as the previous: colour reused).
        let view = crate::environ::WorldView { grid: w.grid, min_y: w.min_y, height: w.height };
        let mut last: Option<(u32, [i32; 3], u32)> = None;
        for mut q in self.pending.drain(..) {
            let n = self.quads.len();
            let accepted = n < CAPACITY
                && (n < CROWDED || {
                    let f = (CAPACITY - n) as f32 / 4096.0;
                    self.rng.f() < f * f
                });
            if accepted {
                if q.biome != 0 {
                    let o = (q.biome >> 8) as i32;
                    let b = q.pos.map(|v| v.floor() as i32);
                    let at = [b[0] + (o & 3) - 1, b[1] + ((o >> 2) & 3) - 1, b[2] + ((o >> 4) & 3) - 1];
                    let c = match last {
                        Some((k, p, c)) if k == q.biome && p == at => c,
                        _ => {
                            let c = biome_color(&view, &self.biomes, (q.biome & 255) as u8, at);
                            last = Some((q.biome, at, c));
                            c
                        }
                    };
                    for (v, k) in q.rgba.iter_mut().zip(rgb_of(c)) {
                        *v *= k;
                    }
                    q.biome = 0;
                }
                self.quads.push(q);
            } else if q.kind.limited() {
                // Refused by the queue: the category slot is given back.
                self.limited = self.limited.saturating_sub(1);
            }
        }
        self.pending.clear();
        self.stats.tick_s += t.elapsed().as_secs_f64();
        self.stats.ticks += 1;
        if self.report && self.stats.ticks >= 20 {
            let s = &self.stats;
            eprintln!(
                "[{:>6.1} s] particles: {} alive, tick {:.3} ms ({}), preparation {:.3} ms/frame",
                self.started.elapsed().as_secs_f64(),
                self.quads.len(),
                s.tick_s * 1e3 / f64::from(s.ticks),
                if par { "pool" } else { "sequential" },
                s.prepare_s * 1e3 / f64::from(s.frames.max(1)),
            );
            self.stats = Stats::default();
        }
    }

    /// Test emissions (demonstration, measurement load) in front of eyes `eye`, yaw `yaw`.
    pub fn test_emit(&mut self, eye: [f64; 3], yaw: f32) {
        let (s, c) = (f64::from(yaw.to_radians().sin()), f64::from(yaw.to_radians().cos()));
        // Frame: forward (−sin, cos), right (−cos, −sin) seen from above.
        let at =
            |right: f64, up: f64, fwd: f64| [eye[0] - s * fwd - c * right, eye[1] + up, eye[2] + c * fwd - s * right];
        if self.bench > 0 {
            let kinds = [K::Smoke, K::Dust, K::Poof, K::Flame, K::Snowflake, K::Cloud, K::HappyVillager, K::Block];
            let o = Options { state: 1, ..Options::default() };
            let mut i = 0;
            while self.quads.len() + self.pending.len() < self.bench {
                let (a, b, d) = (self.rng.d(), self.rng.d(), self.rng.d());
                let pos = at((a - 0.5) * 16.0, (b - 0.5) * 6.0, 4.0 + d * 12.0);
                self.spawn_direct(kinds[i % kinds.len()], &o, pos, [0.0; 3]);
                i += 1;
            }
        }
        let Some(list) = self.demo.clone() else { return };
        self.demo_ticks += 1;
        let t = self.demo_ticks;
        for (n, &k) in list.iter().enumerate() {
            let Some(&(_, period, param)) = DEMO.iter().find(|d| d.0 == k) else { continue };
            if !t.is_multiple_of(period) {
                continue;
            }
            let per_row = list.len().min(10);
            let (col, row) = ((n % 10) as f64, (n / 10) as f64);
            let rows = list.len().div_ceil(10) as f64;
            let pos = at((col - (per_row as f64 - 1.0) / 2.0) * 1.6, (rows - 1.0) * 0.85 - 1.0 - row * 1.7, 9.0);
            let o = Options {
                state: 1,
                item: 1,
                rgb: [0.2, 0.4, 1.0],
                argb: 0xff_ff_40_40,
                force: true,
                ..Options::default()
            };
            match k {
                K::Portal | K::Enchant => {
                    for _ in 0..2 {
                        let g = [self.rng.gauss(), self.rng.gauss(), self.rng.gauss()];
                        self.spawn(k, &o, pos, [g[0] * 0.5, g[1] * 0.5 - 0.5, g[2] * 0.5]);
                    }
                }
                K::Block | K::Item => {
                    for _ in 0..3 {
                        let v = [self.rng.d() - 0.5, self.rng.d() - 0.5, self.rng.d() - 0.5];
                        self.spawn(k, &o, pos, v);
                    }
                }
                K::Firework => {
                    // The five shapes in turn, two colours, blue fade.
                    let shape = (t / period % 5) as u8;
                    let e = FireworkExplosion {
                        shape,
                        colors: vec![0xff_4040, 0x40_ff40],
                        fades: vec![0x40_40ff],
                        trail: shape.is_multiple_of(2),
                        twinkle: shape == 1,
                    };
                    self.create_fireworks([pos[0], pos[1] + 6.0, pos[2]], [0.0, 0.5, 0.0], vec![e], true);
                }
                _ => {
                    self.spawn(k, &o, pos, param);
                }
            }
        }
    }

    /// Instances of the frame: interpolation, size, sprite, colour and light; opaque then
    /// translucent, in insertion order. `in_view`: box (relative to the origin) visible.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        out: &mut ParticleRenderer,
        w: &TickWorld<'_>,
        eye: [f64; 3],
        yaw: f32,
        pitch: f32,
        pt: f32,
        in_view: &dyn Fn([f32; 3], [f32; 3]) -> bool,
    ) {
        let t0 = Instant::now();
        if let Some(img) = self.assets.image.take() {
            out.pending_atlas = Some(img);
        }
        let origin = [eye[0].floor(), eye[2].floor()];
        let (right, up, _) = crate::camera::axes(yaw, pitch);
        out.right_up = [right.extend(0.0).to_array(), up.extend(0.0).to_array()];
        out.opaque.clear();
        out.trans.clear();
        let a = &self.assets;
        let pt64 = f64::from(pt);
        for p in &self.quads {
            let rel = |v: [f64; 3]| [(v[0] - origin[0]) as f32, v[1] as f32, (v[2] - origin[1]) as f32];
            let now = rel(p.pos);
            if !in_view(now, now) {
                continue;
            }
            if p.kind == K::Shriek && p.param[0] > 0.0 || p.hidden() {
                continue;
            }
            let (size, alpha) = p.render_size(pt);
            if size <= 0.0 || alpha <= 0.0 {
                continue;
            }
            let lerp = [0, 1, 2].map(|i| p.old[i] + (p.pos[i] - p.old[i]) * pt64);
            let uv = if p.block_tex > 0 {
                let s = a.block_size;
                [p.uv[0] * s, p.uv[1] * s, p.uv[2] * s, p.uv[3] * s]
            } else {
                let r = a.rects.get(p.sprite as usize).copied().unwrap_or([0.0; 4]);
                let puv = if p.kind == K::Vibration { strip_frame(r, self.anim_clock) } else { p.uv };
                [r[0] + puv[0] * r[2], r[1] + puv[1] * r[3], r[0] + puv[2] * r[2], r[1] + puv[3] * r[3]]
            };
            // Orientation (bits 29 to 31 of `tex`): 0 camera-facing, 1 and 2 planes of the shrieker's
            // shriek, 3 camera-facing without tilting (fixed vertical axis), 4 and 5 planes of the
            // vibration signal (heading in the roll, pitch and sway on 14 bits each).
            let tex = if p.block_tex > 0 {
                match out.layers.get((p.block_tex - 1) as usize) {
                    Some(&l) if l != u32::MAX => l + 1,
                    _ => continue,
                }
            } else {
                match p.kind {
                    K::Shriek => 1 << 29,
                    K::TrialDetection | K::TrialDetectionOminous => 3 << 29,
                    K::Vibration => 4 << 29 | p.vib_angles(pt),
                    _ => 0,
                }
            };
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u32;
            let mut rgb = [p.rgba[0], p.rgba[1], p.rgba[2]];
            if p.kind == K::DustColorTransition {
                // Blend start → end over `(age + t)/(life + 1)`.
                let k = (p.age as f32 + pt) / (p.life as f32 + 1.0);
                for (v, to) in rgb.iter_mut().zip(p.param) {
                    *v += (to as f32 - *v) * k;
                }
            }
            let inst = Inst {
                pos: rel(lerp),
                size,
                uv,
                color: c(rgb[0]) | c(rgb[1]) << 8 | c(rgb[2]) << 16 | c(alpha) << 24,
                light: p.light(w, pt),
                roll: p.old_roll + (p.roll - p.old_roll) * pt,
                tex,
            };
            let second = match p.kind {
                K::Shriek => Some(Inst { tex: 2 << 29, ..inst }),
                K::Vibration => Some(Inst { tex: inst.tex + (1 << 29), ..inst }),
                _ => None,
            };
            if p.translucent || alpha < 1.0 {
                out.trans.push(inst);
                out.trans.extend(second);
            } else {
                out.opaque.push(inst);
                out.opaque.extend(second);
            }
        }
        self.stats.prepare_s += t0.elapsed().as_secs_f64();
        self.stats.frames += 1;
    }
}

/// Demonstration types: emission period (ticks) and velocity-parameter.
const DEMO: &[(Kind, u32, [f64; 3])] = &[
    (K::Block, 5, [0.0; 3]),
    (K::Item, 5, [0.0; 3]),
    (K::Poof, 4, [0.0; 3]),
    (K::Smoke, 2, [0.0; 3]),
    (K::LargeSmoke, 4, [0.0; 3]),
    (K::Flame, 2, [0.0; 3]),
    (K::SoulFireFlame, 2, [0.0; 3]),
    (K::Lava, 8, [0.0; 3]),
    (K::DrippingWater, 10, [0.0; 3]),
    (K::DrippingLava, 10, [0.0; 3]),
    (K::Splash, 2, [0.0; 3]),
    (K::Bubble, 2, [0.0; 3]),
    (K::BubblePop, 4, [0.0; 3]),
    (K::Rain, 2, [0.0; 3]),
    (K::Crit, 2, [0.0, 0.5, 0.0]),
    (K::EnchantedHit, 2, [0.0, 0.5, 0.0]),
    (K::DamageIndicator, 4, [0.0, 0.0, 0.0]),
    (K::Heart, 6, [0.0; 3]),
    (K::AngryVillager, 6, [0.0; 3]),
    (K::HappyVillager, 2, [0.0; 3]),
    (K::Note, 6, [0.3, 0.0, 0.0]),
    (K::Portal, 1, [0.0; 3]),
    (K::Enchant, 1, [0.0; 3]),
    (K::Explosion, 6, [0.5, 0.0, 0.0]),
    (K::ExplosionEmitter, 40, [0.0; 3]),
    (K::Cloud, 3, [0.0; 3]),
    (K::Dust, 2, [0.0; 3]),
    (K::EntityEffect, 2, [0.0; 3]),
    (K::Witch, 2, [0.0; 3]),
    (K::CampfireCosySmoke, 6, [0.0, 0.07, 0.0]),
    (K::Ash, 2, [0.0; 3]),
    (K::WhiteAsh, 2, [0.0; 3]),
    (K::FallingDust, 3, [0.0; 3]),
    (K::Snowflake, 2, [0.0; 3]),
    (K::Glow, 2, [0.0; 3]),
    (K::TotemOfUndying, 2, [0.0, 0.3, 0.0]),
    (K::SweepAttack, 6, [0.0; 3]),
    (K::EndRod, 2, [0.0, 0.05, 0.0]),
    (K::WhiteSmoke, 2, [0.0, 0.02, 0.0]),
    (K::DragonBreath, 2, [0.0, 0.05, 0.0]),
    (K::SculkCharge, 2, [0.0; 3]),
    (K::SculkChargePop, 2, [0.02, 0.02, 0.0]),
    (K::Shriek, 12, [0.0; 3]),
    (K::TrialDetection, 2, [0.0; 3]),
    (K::TrialDetectionOminous, 2, [0.0; 3]),
    (K::VaultConnection, 2, [0.5, 0.5, 0.0]),
    (K::TrialOmen, 2, [0.0; 3]),
    (K::RaidOmen, 2, [0.0; 3]),
    (K::OminousSpawning, 2, [0.5, 0.5, 0.0]),
    (K::DrippingDripstoneWater, 10, [0.0; 3]),
    (K::DrippingDripstoneLava, 10, [0.0; 3]),
    (K::Underwater, 1, [0.0; 3]),
    (K::SporeBlossomAir, 2, [0.0; 3]),
    (K::CrimsonSpore, 1, [0.0; 3]),
    (K::WarpedSpore, 1, [0.0; 3]),
    (K::FallingSporeBlossom, 4, [0.0; 3]),
    (K::DrippingHoney, 10, [0.0; 3]),
    (K::DrippingObsidianTear, 10, [0.0; 3]),
    (K::ReversePortal, 2, [0.0, 0.02, 0.0]),
    (K::CherryLeaves, 6, [0.0; 3]),
    (K::PaleOakLeaves, 6, [0.0; 3]),
    (K::RedPoplarLeaves, 6, [0.0; 3]),
    (K::TintedLeaves, 6, [0.0; 3]),
    (K::Firefly, 6, [0.0; 3]),
    (K::DustColorTransition, 2, [0.0, 0.02, 0.0]),
    (K::BlockMarker, 80, [0.0; 3]),
    (K::Firework, 40, [0.0; 3]),
    (K::SonicBoom, 16, [0.0; 3]),
    (K::Gust, 12, [0.0; 3]),
    (K::SmallGust, 4, [0.0; 3]),
    (K::GustEmitterLarge, 20, [0.0; 3]),
    (K::SquidInk, 3, [0.0, 0.1, 0.0]),
    (K::GlowSquidInk, 3, [0.0, 0.1, 0.0]),
    (K::Soul, 2, [0.0, 0.02, 0.0]),
    (K::SculkSoul, 2, [0.0, 0.02, 0.0]),
    (K::Spit, 2, [0.0, 0.1, 0.0]),
    (K::Fishing, 2, [0.0, 0.05, 0.0]),
    (K::DustPlume, 2, [0.0; 3]),
    (K::Infested, 2, [0.0; 3]),
    (K::PauseMobGrowth, 2, [0.0; 3]),
    (K::ResetMobGrowth, 2, [0.0; 3]),
    (K::NoxiousGas, 4, [0.0; 3]),
    (K::SulfurCubeGoo, 4, [0.0; 3]),
];

// ---------- Rendering ----------

/// Instance: position (relative to the camera origin), half-side, texture rectangle in
/// pixels (u0, v0, u1, v1), 8-bit RGBA colour, light, roll, texture (0 particle atlas,
/// otherwise block texture layer + 1).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Inst {
    pos: [f32; 3],
    size: f32,
    uv: [f32; 4],
    color: u32,
    light: u32,
    roll: f32,
    tex: u32,
}

/// GPU side: two pipelines (opaque with the entities, translucent after the translucent
/// terrain), particle atlas, one reused instance buffer.
pub struct ParticleRenderer {
    opaque_pl: wgpu::RenderPipeline,
    trans_pl: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    axes: wgpu::Buffer,
    bind: Option<wgpu::BindGroup>,
    pending_atlas: Option<Image>,
    /// Block texture → layer of the main array (`u32::MAX`: absent).
    layers: Vec<u32>,
    opaque: Vec<Inst>,
    trans: Vec<Inst>,
    buf: Option<(wgpu::Buffer, u64)>,
    counts: (u32, u32),
    right_up: [[f32; 4]; 2],
    /// Camera layout (group 0), for the pipelines made on demand.
    camera_layout: wgpu::BindGroupLayout,
    /// Translucent particles, improved transparency.
    oit: Option<crate::oit::Set>,
}

impl ParticleRenderer {
    /// Pipelines of the translucent particles for improved transparency (once): culled
    /// back faces, like the game's pipeline.
    pub fn ensure_oit(&mut self, device: &wgpu::Device, layouts: &crate::oit::Layouts) {
        if self.oit.is_some() {
            return;
        }
        let module = crate::oit::module(
            device,
            "particles (transparency)",
            include_str!("particles.wgsl"),
            include_str!("particles_oit.wgsl"),
        );
        let groups = [Some(&self.camera_layout), Some(&self.layout)];
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Inst>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Unorm8x4,
                4 => Uint32, 5 => Float32, 6 => Uint32
            ],
        })];
        let spec =
            crate::oit::Spec::new("particles (transparency)", &groups, &module, "vs", &buffers, "fs").cull(true);
        self.oit = Some(layouts.set(device, &spec));
    }

    /// Translucents, stage `f` of the improved transparency; group 0 (camera) is already bound.
    pub fn draw_translucent_oit(&self, pass: &mut wgpu::RenderPass<'_>, f: crate::oit::Frame<'_>) {
        let (Some(bind), Some((buf, _)), Some(set)) = (&self.bind, &self.buf, &self.oit) else { return };
        if self.counts.1 == 0 {
            return;
        }
        f.set(pass, set);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, self.counts.0..self.counts.0 + self.counts.1);
    }

    pub fn new(
        device: &wgpu::Device,
        camera_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        layers: Vec<u32>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles"),
            source: wgpu::ShaderSource::Wgsl(include_str!("particles.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particles"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("particles"),
            bind_group_layouts: &[Some(camera_layout), Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |label, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Inst>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32, 2 => Float32x4, 3 => Unorm8x4,
                            4 => Uint32, 5 => Float32, 6 => Uint32
                        ],
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(crate::render::DEPTH_NEARER),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let opaque_pl = pipeline("opaque particles", None);
        let trans_pl = pipeline("translucent particles", Some(wgpu::BlendState::ALPHA_BLENDING));
        let axes = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles (axes)"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ParticleRenderer {
            opaque_pl,
            trans_pl,
            layout,
            axes,
            bind: None,
            pending_atlas: None,
            layers,
            opaque: Vec::new(),
            trans: Vec::new(),
            buf: None,
            counts: (0, 0),
            right_up: [[0.0; 4]; 2],
            camera_layout: camera_layout.clone(),
            oit: None,
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(img) = self.pending_atlas.take() {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("particle atlas"),
                size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                tex.as_image_copy(),
                &img.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(img.width * 4),
                    rows_per_image: Some(img.height),
                },
                wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            );
            let view = tex.create_view(&Default::default());
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("particles"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: self.axes.as_entire_binding() },
                ],
            }));
        }
        self.counts = (self.opaque.len() as u32, self.trans.len() as u32);
        if self.counts == (0, 0) {
            return;
        }
        queue.write_buffer(&self.axes, 0, bytemuck::cast_slice(&self.right_up));
        let size = std::mem::size_of::<Inst>() as u64;
        let need = (u64::from(self.counts.0) + u64::from(self.counts.1)) * size;
        if self.buf.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two().max(16 * 1024);
            self.buf = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("particles"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        let buf = &self.buf.as_ref().expect("created above").0;
        if !self.opaque.is_empty() {
            queue.write_buffer(buf, 0, bytemuck::cast_slice(&self.opaque));
        }
        if !self.trans.is_empty() {
            queue.write_buffer(buf, u64::from(self.counts.0) * size, bytemuck::cast_slice(&self.trans));
        }
    }

    /// Opaques, with the entities; group 0 (camera) is already bound.
    pub fn draw_opaque(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.opaque_pl, 0..self.counts.0);
    }

    /// Translucents, after the translucent terrain.
    pub fn draw_translucent(&self, pass: &mut wgpu::RenderPass<'_>) {
        self.draw(pass, &self.trans_pl, self.counts.0..self.counts.0 + self.counts.1);
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pipeline: &wgpu::RenderPipeline, range: std::ops::Range<u32>) {
        let (Some(bind), Some((buf, _))) = (&self.bind, &self.buf) else { return };
        if range.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        pass.set_bind_group(1, bind, &[]);
        pass.set_vertex_buffer(0, buf.slice(..));
        pass.draw(0..6, range);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets() -> ParticleAssets {
        ParticleAssets {
            rects: vec![[0.0, 0.0, 16.0, 16.0]],
            translucent: vec![false],
            sets: vec![vec![0]; KINDS.len()],
            blocks: vec![
                BlockPart { tex: u32::MAX, none: true, invisible: false },
                BlockPart { tex: 0, none: false, invisible: false },
            ],
            state_block: vec![0, 1],
            items: vec![vec![0]],
            snowball: 0,
            slime: 0,
            cobweb: 0,
            block_size: 16.0,
            image: None,
            markers: Vec::new(),
        }
    }

    fn plain(a: &ParticleAssets) -> Make<'_> {
        Make { a, terrain: &[], dust: &[], scope: None }
    }

    #[test]
    fn names_round_trip() {
        for &(k, n) in KINDS {
            assert_eq!(Kind::from_name(n), Some(k));
            assert_eq!(Kind::from_name(&format!("minecraft:{n}")), Some(k));
            assert_eq!(k.name(), n);
        }
    }

    #[test]
    fn base_velocity_norm_ignores_parameter() {
        let a = assets();
        let mut spell = Rng(3);
        for i in 0..200 {
            let q = create(
                K::Block,
                &Options { state: 1, ..Default::default() },
                [0.0; 3],
                [5.0, -3.0, 2.0],
                Rng(i),
                &mut spell,
                &plain(&a),
            )
            .expect("block");
            let n = len3([q.vel[0], q.vel[1] - 0.1, q.vel[2]]);
            assert!((0.06 - 1e-9..=0.18 + 1e-9).contains(&n), "norm {n}");
            assert!((4..=40).contains(&q.life));
        }
        // Air: refused.
        assert!(create(K::Block, &Options::default(), [0.0; 3], [0.0; 3], Rng(1), &mut spell, &plain(&a)).is_none());
    }

    #[test]
    fn standard_tick_without_world() {
        let a = assets();
        let mut spell = Rng(1);
        let mut q =
            create(K::Poof, &Options::default(), [0.0, 64.0, 0.0], [0.0; 3], Rng(9), &mut spell, &plain(&a)).unwrap();
        let life = q.life;
        let mut ticks = 0;
        while !q.removed {
            q.tick(&a, None);
            ticks += 1;
        }
        // The particle lives `duration` ticks of movement, dies at the next; it rises (G −0.1).
        assert_eq!(ticks, life + 1);
        assert!(q.pos[1] > 64.0);
    }

    #[test]
    fn queue_admission_and_capacity() {
        let a = assets();
        let data_free = Particles {
            assets: a,
            quads: Vec::new(),
            vib_tracking: false,
            anim_clock: 0,
            pending: Vec::new(),
            emitters: Vec::new(),
            rng: Rng(7),
            level: 0,
            spell: Rng(8),
            camera: [0.0; 3],
            by_id: Vec::new(),
            children: Vec::new(),
            limited: 0,
            sounds: Vec::new(),
            starters: Vec::new(),
            demo: None,
            demo_ticks: 0,
            bench: 0,
            parallel: Some(false),
            stats: Stats::default(),
            report: false,
            started: Instant::now(),
            terrain: Vec::new(),
            dust: Vec::new(),
            scope: None,
            biomes: (std::sync::Arc::new(Vec::new()), 2, 0),
            gas: GasTables::new(&rc_data::VersionData::default()),
        };
        let mut p = data_free;
        // Beyond 32 blocks: refused except forced type.
        assert!(!p.spawn(K::Heart, &Options::default(), [40.0, 0.0, 0.0], [0.0; 3]));
        assert!(p.spawn(K::Poof, &Options::default(), [40.0, 0.0, 0.0], [0.0; 3]));
        for _ in 0..20_000 {
            p.spawn_direct(K::Heart, &Options::default(), [0.0; 3], [0.0; 3]);
        }
        let grid = ChunkGrid::new(2);
        let data = rc_data::VersionData::default();
        let phys = rc_physics::Tables::new(&data);
        let w = TickWorld { grid: &grid, min_y: 0, height: 256, phys: &phys, player: None, scope: None };
        p.tick(&w, [0.0; 3]);
        assert!(p.len() > CROWDED && p.len() <= CAPACITY, "{}", p.len());
    }

    /// A column (0, 0) of a section (y 0 to 15), single biome, blocks placed.
    fn column(biome: u32, blocks: &[([usize; 3], u32)], bits: u8) -> rc_world::Column {
        let mut s = rc_world::Section::empty(0, biome);
        let changes: Vec<(u16, u32)> =
            blocks.iter().map(|&(p, st)| (rc_world::block_index(p[0], p[1], p[2]) as u16, st)).collect();
        s.set_blocks(&changes, bits, |v| v == 0);
        rc_world::Column {
            x: 0,
            z: 0,
            sections: vec![s].into_boxed_slice(),
            sky_light: vec![None; 3].into_boxed_slice(),
            block_light: vec![None; 3].into_boxed_slice(),
            heightmaps: Vec::new(),
        }
    }

    fn engine(data: &rc_data::VersionData) -> Particles {
        let mut p = Particles::new(assets(), data);
        p.parallel = Some(false);
        p.rng = Rng(0x5eed_1234);
        p
    }

    /// Block fragments: 0.6 × the particle tint of the state (grass: none, redstone wire
    /// by power), 0.6 grey without tint; falling dust: colour of the state, × 1.
    #[test]
    fn block_chunks_and_dust_take_state_tints() {
        use rc_data::tint::{CONSTANT, NONE};
        let a = assets();
        let terrain = [(NONE, 0), (CONSTANT, 0xffff_ffff)];
        let dust = [(CONSTANT, 0), (CONSTANT, 0xffdb_d3a0)];
        let m = Make { a: &a, terrain: &terrain, dust: &dust, scope: None };
        let mut spell = Rng(1);
        let o = Options { state: 1, ..Default::default() };
        let q = create(K::Block, &o, [0.0; 3], [0.0; 3], Rng(2), &mut spell, &m).unwrap();
        assert_eq!(q.rgba[..3], [0.6; 3]);
        let red = [(NONE, 0), (CONSTANT, 0xffff_3200)];
        let q = create(K::Block, &o, [0.0; 3], [0.0; 3], Rng(2), &mut spell, &Make { terrain: &red, ..m }).unwrap();
        assert_eq!(q.rgba[..3], [0.6, 0.6 * (50.0 / 255.0), 0.0]);
        let none = [(NONE, 0), (NONE, 0)];
        let q = create(K::Block, &o, [0.0; 3], [0.0; 3], Rng(2), &mut spell, &Make { terrain: &none, ..m }).unwrap();
        assert_eq!((q.rgba[..3].to_vec(), q.biome), (vec![0.6; 3], 0));
        let q = create(K::FallingDust, &o, [0.0; 3], [0.0; 3], Rng(2), &mut spell, &m).unwrap();
        assert_eq!(q.rgba[..3], rgb_of(0xdb_d3a0));
    }

    /// Biome tint (water, grass…) resolved on entering the queue: biome of the offset creation
    /// block, blended over 5 × 5; colour × 0.6.
    #[test]
    fn biome_tint_resolved_on_entry() {
        let data = rc_data::VersionData::default();
        let mut p = engine(&data);
        p.terrain = vec![(0, 0), (rc_data::tint::WATER, 0x15)];
        let palette = std::sync::Arc::new(vec![[0x11_1111; 4], [0x80_b040, 0, 0, 0x30_60c0]]);
        p.set_biomes(&palette, 2, 0);
        let mut grid = ChunkGrid::new(2);
        grid.insert(column(1, &[], 15));
        let phys = rc_physics::Tables::new(&data);
        let w = TickWorld { grid: &grid, min_y: 0, height: 16, phys: &phys, player: None, scope: None };
        let pos = [8.5, 8.5, 8.5];
        assert!(p.spawn(K::Block, &Options { state: 1, ..Default::default() }, pos, [0.0; 3]));
        p.tick(&w, pos);
        let q = p.quads.iter().find(|q| q.kind == K::Block).expect("fragment admitted");
        assert_eq!(q.rgba[..3], rgb_of(0x30_60c0).map(|c| 0.6 * c));
        assert_eq!(q.biome, 0);
    }

    /// Bench (`cargo test --release -p rc-app -- --ignored bench_fragment_tint --nocapture`): cost of
    /// the queue entry of 64 block fragments per break, fixed tint against biome tint.
    #[test]
    #[ignore]
    fn bench_fragment_tint() {
        let data = rc_data::VersionData::default();
        let palette = std::sync::Arc::new(vec![[0x11_1111; 4], [0x80_b040, 0, 0, 0x30_60c0]]);
        let mut grid = ChunkGrid::new(2);
        grid.insert(column(1, &[], 15));
        let phys = rc_physics::Tables::new(&data);
        let w = TickWorld { grid: &grid, min_y: 0, height: 16, phys: &phys, player: None, scope: None };
        for (name, t) in [("fixed", (rc_data::tint::CONSTANT, 0x80_b040)), ("biome", (rc_data::tint::WATER, 0x15))] {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let mut p = engine(&data);
                p.terrain = vec![(0, 0), t];
                p.set_biomes(&palette, 2, 0);
                let o = Options { state: 1, ..Default::default() };
                let start = Instant::now();
                for b in 0..200 {
                    let base = [2.0 + f64::from(b % 12), 4.0, 2.0 + f64::from(b / 12 % 12)];
                    for i in 0..64 {
                        let d = [i & 3, (i >> 2) & 3, i >> 4].map(|v| (f64::from(v) + 0.5) / 4.0);
                        p.spawn_direct(K::Block, &o, [base[0] + d[0], base[1] + d[1], base[2] + d[2]], [0.0; 3]);
                    }
                    p.tick(&w, [8.0, 8.0, 8.0]);
                    p.quads.clear();
                }
                best = best.min(start.elapsed().as_secs_f64());
            }
            eprintln!("tint {name}: {:.2} µs per break (64 fragments, creation + entry)", best * 1e6 / 200.0);
        }
    }

    /// Small flame: box of 0.1 re-centred on the old box (the random offset of the
    /// position is lost at the first movement, like the large flame).
    #[test]
    fn small_flame_box_stays_centered() {
        let a = assets();
        let mut spell = Rng(1);
        let q = create(K::SmallFlame, &Options::default(), [3.0, 4.0, 5.0], [0.0; 3], Rng(4), &mut spell, &plain(&a))
            .unwrap();
        assert_eq!((q.bw, q.bh), (0.1, 0.1));
        assert!((q.bmin[0] + q.bw / 2.0 - 3.0).abs() < 1e-12 && (q.bmin[2] + q.bw / 2.0 - 5.0).abs() < 1e-12);
        assert_eq!(q.bmin[1], 4.0);
    }

    /// `block_marker` ignores distance and setting (forced type).
    #[test]
    fn block_marker_is_forced() {
        let mut p = engine(&rc_data::VersionData::default());
        p.level = 2;
        assert!(p.spawn(K::BlockMarker, &Options { state: 1, ..Default::default() }, [100.0, 0.0, 0.0], [0.0; 3]));
        assert!(!p.spawn(K::Heart, &Options::default(), [1.0, 0.0, 0.0], [0.0; 3]));
    }

    /// `dust_color_transition` received: end colour and scale of the packet.
    #[test]
    fn transition_target_from_network() {
        let net = rc_proto_v777::entity::Particle {
            kind: 0,
            color: 0x00ff_0000,
            state: 0,
            item: -1,
            scale: 2.0,
            color2: 0x0000_ff00,
            vib: None,
        };
        let o = Options::from_net(&net, false);
        assert_eq!((o.rgb, o.rgb2, o.scale), ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 2.0));
    }

    /// Spells within 3 blocks at most of the player aiming with a spyglass: invisible at creation
    /// (original alpha included, except the entity effect which restores its own) and at each tick.
    #[test]
    fn spells_hidden_near_scoping_player() {
        let a = assets();
        let mut spell = Rng(1);
        let eye = Some([0.0, 2.0, 0.0]);
        let near = Make { a: &a, terrain: &[], dust: &[], scope: eye };
        let q = create(K::Witch, &Options::default(), [0.0, 0.0, 0.0], [0.0; 3], Rng(3), &mut spell, &near).unwrap();
        assert_eq!((q.rgba[3], q.alpha0), (0.0, 0.0));
        let far = create(K::Witch, &Options::default(), [0.0, 5.5, 0.0], [0.0; 3], Rng(3), &mut spell, &near).unwrap();
        assert_eq!(far.rgba[3], 1.0);
        let o = Options { argb: 0x80ff_ffff, ..Default::default() };
        let mut e = create(K::EntityEffect, &o, [0.0; 3], [0.0; 3], Rng(3), &mut spell, &near).unwrap();
        assert_eq!(e.rgba[3], 128.0 / 255.0);
        let grid = ChunkGrid::new(1);
        let data = rc_data::VersionData::default();
        let phys = rc_physics::Tables::new(&data);
        let w = TickWorld { grid: &grid, min_y: 0, height: 16, phys: &phys, player: None, scope: eye };
        e.tick(&a, Some(&w));
        assert_eq!(e.rgba[3], 0.0);
        let w = TickWorld { scope: None, ..w };
        e.tick(&a, Some(&w));
        assert!((e.rgba[3] - 0.05 * 128.0 / 255.0).abs() < 1e-6);
    }

    /// Gas exit: water sources that let through are crossed, the first cell that
    /// is not one comes out if it lets through, otherwise nothing; 5 cells at most.
    #[test]
    fn gas_source_above_sulfur() {
        // States: 0 air, 1 water, 2 stone, 3 kelp (water source without collision).
        let g = GasTables {
            water_block: vec![false, true, false, false],
            water_source: vec![false, true, false, true],
            open: vec![true, true, false, true],
        };
        let col = |c: &'static [u32]| move |_: i32, y: i32, _: i32| c.get(y as usize).copied().unwrap_or(0);
        assert_eq!(g.source(col(&[9, 1, 3, 1, 0]), [0, 0, 0]), Some([0, 4, 0]));
        assert_eq!(g.source(col(&[9, 1, 2]), [0, 0, 0]), None);
        assert_eq!(g.source(col(&[9, 0]), [0, 0, 0]), Some([0, 1, 0]));
        assert_eq!(g.source(col(&[9, 1, 1, 1, 1, 1, 0]), [0, 0, 0]), None);
    }

    /// Harmful gas cloud (26.3 data): 20 ticks, one attempt every other tick; above a
    /// water body, each attempt gives a gas; under a stone lid, none.
    #[test]
    fn noxious_gas_cloud_emits_over_water() {
        let d = rc_data::load_cached("26.3").expect("26.3 data in cache (prepare, or RUSTCRAFT_CACHE)");
        let id = |n: &str| d.blocks.iter().find(|b| b.name == n).expect(n).default_state;
        let (water, stone) = (id("minecraft:water"), id("minecraft:stone"));
        let phys = rc_physics::Tables::new(&d);
        let run = |lid: bool| {
            let mut blocks = Vec::new();
            for x in 3..14 {
                for z in 3..14 {
                    for y in 1..4 {
                        blocks.push(([x, y, z], water));
                    }
                    if lid {
                        blocks.push(([x, 4, z], stone));
                    }
                }
            }
            let mut grid = ChunkGrid::new(2);
            grid.insert(column(0, &blocks, d.state_bits() as u8));
            let w = TickWorld { grid: &grid, min_y: 0, height: 16, phys: &phys, player: None, scope: None };
            let mut p = engine(&d);
            let at = [8.5, 4.5, 8.5];
            assert!(p.spawn(K::NoxiousGasCloud, &Options::default(), at, [0.0; 3]));
            let mut gas = 0;
            for _ in 0..25 {
                p.tick(&w, at);
                gas += p.quads.iter().filter(|q| q.kind == K::NoxiousGas && q.age == 0).count();
            }
            (gas, p.emitters.len())
        };
        assert_eq!(run(false), (10, 0));
        assert_eq!(run(true).0, 0);
    }

    /// G6-F10: vibration signal towards a block: created only with its destination, duration =
    /// trip, brought closer by a fraction 1/(remaining ticks), removed at the last tick; heading and
    /// pitch towards the target; animated strip cut by the common clock; angles quantised to within 1e-4.
    #[test]
    fn vibration_signal_flies_to_its_block() {
        use rc_proto_v777::entity::VibDest;
        let mut a = assets();
        // Strip of 4 frames of 16 × 16.
        a.rects = vec![[0.0, 0.0, 16.0, 64.0]];
        let m = plain(&a);
        let mut spell = Rng(1);
        assert!(create(K::Vibration, &Options::default(), [0.0; 3], [0.0; 3], Rng(2), &mut spell, &m).is_none());
        let o = Options { vib: Some((VibDest::Block([4, 0, 0]), 4)), ..Default::default() };
        let start = [0.5, 0.5, 0.5];
        let mut q = create(K::Vibration, &o, start, [0.0; 3], Rng(2), &mut spell, &m).expect("signal");
        assert_eq!((q.life, q.size, q.start), (4, 0.3, [4.5, 0.5, 0.5]));
        // Heading: atan2(dx, dz) with dx = −4, dz = 0 → −π/2; zero pitch.
        assert!((q.roll + std::f32::consts::FRAC_PI_2).abs() < 1e-6 && q.param[0] == 0.0);
        let mut xs = Vec::new();
        let uv0 = q.uv;
        for _ in 0..4 {
            q.tick(&a, None);
            if q.removed {
                break;
            }
            xs.push(q.pos[0]);
            // The frame does not depend on the age (common clock, chosen at render).
            assert_eq!(q.uv, uv0, "frame independent of age");
        }
        // Strip: frame `clock mod 4`, the same for all the particles at the same tick.
        let r = a.rects[0];
        for clock in [0u32, 1, 3, 4, 9] {
            let f = (clock % 4) as f32;
            assert_eq!(strip_frame(r, clock), [0.0, f / 4.0, 1.0, (f + 1.0) / 4.0], "clock {clock}");
        }
        // The game's vibration.png: 18 × 126, 7 frames.
        assert_eq!(strip_frame([0.0, 0.0, 18.0, 126.0], 13), [0.0, 6.0 / 7.0, 1.0, 1.0]);
        // 3 remaining: +4/3; 2 remaining: half of the rest; 1 remaining: arrived; then removed.
        let want = [0.5 + 4.0 / 3.0, 0.5 + 4.0 / 3.0 + (4.0 - 4.0 / 3.0) / 2.0, 4.5];
        assert_eq!(xs.len(), 3, "removed at the 4th tick");
        for (x, w) in xs.iter().zip(want) {
            assert!((x - w).abs() < 1e-12, "{x} ≠ {w}");
        }
        assert!(q.removed);
        // Angles: pitch and sway decoded like the shader.
        q.param = [0.4, 0.2, 0.0];
        q.age = 3;
        let t = q.vib_angles(0.5);
        let pitch = ((t >> 14) & 0x3fff) as f32 / 16383.0 - 0.5;
        let sway = (t & 0x3fff) as f32 / 16383.0 - 0.5;
        assert!((pitch * std::f32::consts::PI - 0.3).abs() < 1e-3);
        let want_sway = ((3.5 - std::f32::consts::TAU) * 0.05f32).sin() * 2.0;
        assert!((sway * 4.0 - want_sway).abs() < 1e-3);
    }

    /// The packet carries the destination down to the particle's options.
    #[test]
    fn vibration_destination_reaches_options() {
        use rc_proto_v777::entity::VibDest;
        let net = rc_proto_v777::entity::Particle {
            kind: 0,
            color: 0,
            state: 0,
            item: -1,
            scale: 1.0,
            color2: 0,
            vib: Some((VibDest::Entity { id: 7, y_offset: 1.62 }, 9)),
        };
        assert_eq!(Options::from_net(&net, false).vib, net.vib);
    }
}
