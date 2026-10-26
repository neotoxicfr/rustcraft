//! Configured and placed features: registry, data reading, dispatch to the algorithms
//! (`generation-decoration.md` § 5–6).

use hashbrown::HashMap;

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::{BlockId, Dir, State};
use crate::decor_pred::{
    Blockset, FloatP, IntP, ParseCtx, Pred, Provider, Rule, Weighted, get_f32, get_i32, opt_bool, opt_f32, opt_i32,
};
use crate::decor_rng::DecoRng;
use crate::json::Json;
use crate::placement::Modifier;
use crate::tree::{FallenTree, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeatureId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlacedId(pub u32);

#[derive(Debug, Clone)]
pub struct Placed {
    pub feature: FeatureId,
    pub placement: Vec<Modifier>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OreTarget {
    pub rule: Rule,
    pub state: State,
}

#[derive(Debug, Clone)]
pub struct Ore {
    pub targets: Vec<OreTarget>,
    pub size: i32,
    pub discard: f32,
}

#[derive(Debug, Clone)]
pub struct Disk {
    pub provider: Provider,
    pub target: Pred,
    pub radius: IntP,
    pub half_height: i32,
}

#[derive(Debug, Clone)]
pub struct Lake {
    pub fluid: Provider,
    pub barrier: Provider,
    pub can_place: Pred,
    pub can_replace_air_fluid: Pred,
    pub can_replace_barrier: Pred,
}

#[derive(Debug, Clone)]
pub struct Spring {
    /// State of the placed fluid block.
    pub state: State,
    pub requires_below: bool,
    pub rock_count: i32,
    pub hole_count: i32,
    pub valid: Blockset,
}

#[derive(Debug, Clone)]
pub struct BlockColumnLayer {
    pub height: IntP,
    pub provider: Provider,
}

#[derive(Debug, Clone)]
pub struct BlockColumn {
    pub layers: Vec<BlockColumnLayer>,
    pub dir: Dir,
    pub allowed: Pred,
    pub prioritize_tip: bool,
}

#[derive(Debug, Clone)]
pub struct Geode {
    pub filling: Provider,
    pub inner: Provider,
    pub alternate_inner: Provider,
    pub middle: Provider,
    pub outer: Provider,
    pub inner_placements: Vec<State>,
    pub cannot_replace: u16,
    pub invalid_blocks: u16,
    pub filling_l: f64,
    pub inner_l: f64,
    pub middle_l: f64,
    pub outer_l: f64,
    pub crack_chance: f64,
    pub base_crack_size: f64,
    pub crack_point_offset: i32,
    pub use_potential: f64,
    pub use_alternate: f64,
    pub require_alternate: bool,
    pub outer_wall: IntP,
    pub distribution: IntP,
    pub point_offset: IntP,
    pub min_gen: i32,
    pub max_gen: i32,
    pub noise_multiplier: f64,
    pub invalid_threshold: i32,
}

#[derive(Debug, Clone)]
pub struct Multiface {
    pub block: BlockId,
    pub search_range: i32,
    pub floor: bool,
    pub ceiling: bool,
    pub wall: bool,
    pub chance: f32,
    pub can_be_placed_on: Blockset,
}

#[derive(Debug, Clone)]
pub struct VegPatch {
    pub replaceable: u16,
    pub ground: Provider,
    pub vegetation: PlacedId,
    pub ceiling: bool,
    pub depth: IntP,
    pub extra_bottom: f32,
    pub vertical_range: i32,
    pub vegetation_chance: f32,
    pub xz_radius: IntP,
    pub extra_edge: f32,
    pub waterlogged: bool,
}

#[derive(Debug, Clone)]
pub struct RootSystem {
    pub feature: PlacedId,
    pub required_space: i32,
    pub level_test_distance: i32,
    pub max_level_deviation: i32,
    pub root_radius: i32,
    pub root_replaceable: u16,
    pub root_provider: Provider,
    pub root_attempts: i32,
    pub root_column_max_height: i32,
    pub hanging_radius: i32,
    pub hanging_span: i32,
    pub hanging_provider: Provider,
    pub hanging_attempts: i32,
    pub allowed_water: i32,
    pub allowed_tree_position: Pred,
}

#[derive(Debug, Clone)]
pub struct HugeMushroom {
    pub red: bool,
    pub cap: Provider,
    pub stem: Provider,
    pub foliage_radius: i32,
    pub can_place_on: Pred,
}

#[derive(Debug, Clone)]
pub struct Pillar {
    pub block: Provider,
    pub can_replace: Pred,
    pub dir: Dir,
    pub chance: f32,
    pub cap: Option<PlacedId>,
}

#[derive(Debug, Clone)]
pub struct Speleothem {
    pub base: State,
    pub pointed: BlockId,
    pub replaceable: Pred,
    pub taller: f32,
    pub directional: f32,
    pub r2: f32,
    pub r3: f32,
}

#[derive(Debug, Clone)]
pub struct SpeleothemCluster {
    pub base: State,
    pub pointed: BlockId,
    pub replaceable: Pred,
    pub search_range: i32,
    pub height: IntP,
    pub radius: IntP,
    pub max_diff: i32,
    pub height_deviation: i32,
    pub layer_thickness: IntP,
    pub density: FloatP,
    pub wetness: FloatP,
    pub chance_at_max: f32,
    pub max_edge: i32,
    pub max_center: i32,
}

#[derive(Debug, Clone)]
pub struct LargeDripstone {
    pub replaceable: Pred,
    pub search_range: i32,
    pub column_radius: IntP,
    pub height_scale: FloatP,
    pub ratio: f32,
    pub stalactite_bluntness: FloatP,
    pub stalagmite_bluntness: FloatP,
    pub wind_speed: FloatP,
    pub min_radius_for_wind: i32,
    pub min_bluntness_for_wind: f32,
}

#[derive(Debug, Clone)]
pub struct SculkPatch {
    pub charge_count: i32,
    pub amount_per_charge: i32,
    pub spread_attempts: i32,
    pub growth_rounds: i32,
    pub spread_rounds: i32,
}

#[derive(Debug, Clone)]
pub enum Feature {
    NoOp,
    Overlay(Vec<PlacedId>),
    Sequence(Vec<PlacedId>),
    RandomSelector {
        entries: Vec<(f32, PlacedId)>,
        default: PlacedId,
    },
    SimpleRandom(Vec<PlacedId>),
    RandomBoolean(PlacedId, PlacedId),
    WeightedRandom(Weighted<PlacedId>),
    SimpleBlock {
        provider: Provider,
        schedule_tick: bool,
    },
    BlockColumn(Box<BlockColumn>),
    Ore(Box<Ore>),
    ScatteredOre(Box<Ore>),
    Disk(Box<Disk>),
    Lake(Box<Lake>),
    Spring(Box<Spring>),
    UnderwaterMagma {
        range: i32,
        radius: i32,
        prob: f32,
    },
    MonsterRoom,
    Geode(Box<Geode>),
    FreezeTopLayer,
    Multiface(Box<Multiface>),
    VegPatch(Box<VegPatch>),
    RootSystem(Box<RootSystem>),
    BlockPile(Provider),
    Pillar(Box<Pillar>),
    Vines,
    Bamboo(f32),
    BlueIce,
    Iceberg(State),
    Spike {
        state: State,
        can_place_on: Pred,
        can_replace: Pred,
    },
    HugeMushroom(Box<HugeMushroom>),
    CoralTree(PlacedId),
    CoralClaw(PlacedId),
    Tree(Box<Tree>),
    FallenTree(Box<FallenTree>),
    Speleothem(Box<Speleothem>),
    SpeleothemCluster(Box<SpeleothemCluster>),
    LargeDripstone(Box<LargeDripstone>),
    BlockBlob {
        state: State,
        can_place_on: Pred,
    },
    SculkPatch(SculkPatch),
    Delta(Box<crate::feature_nether::Delta>),
    HugeFungus(Box<crate::feature_nether::HugeFungus>),
    PatchySquare(Box<crate::feature_nether::PatchySquare>),
    NeighborSpread(Box<crate::feature_nether::NeighborSpread>),
    ReplaceBlobs(Box<crate::feature_nether::ReplaceBlobs>),
    SteppedColumns(Box<crate::feature_nether::SteppedColumns>),
    EndSpike(Vec<crate::feature_end::Spike>),
    /// Gateway: known exit (`exit`, `exact`) noted in its block entity.
    EndGateway {
        exit: Option<[i32; 3]>,
        exact: bool,
    },
    EndIsland,
    EndPlatform,
    ChorusPlant,
    /// Type recognised but not reproduced yet.
    Template(Box<crate::feature_templ::TemplateCfg>),
    Fossil(Box<crate::feature_templ::FossilCfg>),
    /// Fills the layer `height` (above the world bottom) of the chunk wherever there is air.
    FillLayer {
        height: i32,
        state: State,
    },
    /// Replaces the origin block with the first replacement whose rule passes.
    ReplaceBlock(Vec<OreTarget>),
    /// 33×33 stone platform (cobblestone at the centre) around (8, 3, 8), "the void" preset.
    VoidStartPlatform,
    /// Bonus chest (placed at spawn by the server, not by decoration).
    BonusChest,
    Unsupported(String),
}

/// Registry of the features (configured and placed) read from the data.
#[derive(Debug, Default)]
pub struct Registry {
    pub features: Vec<Feature>,
    pub feature_names: Vec<Option<String>>,
    pub placed: Vec<Placed>,
    feature_by_name: HashMap<String, FeatureId>,
    placed_by_name: HashMap<String, PlacedId>,
}

fn bad(w: impl Into<String>) -> Error {
    Error::Format(w.into())
}

fn ty(j: &Json) -> Result<&str, Error> {
    let t = j.get("type").and_then(Json::as_str).ok_or_else(|| bad("feature without type"))?;
    Ok(t.strip_prefix("minecraft:").unwrap_or(t))
}

impl Registry {
    pub fn placed_id(&self, name: &str) -> Option<PlacedId> {
        crate::data::with_full_id(name, |full| self.placed_by_name.get(full).copied())
    }

    pub fn feature_id(&self, name: &str) -> Option<FeatureId> {
        crate::data::with_full_id(name, |full| self.feature_by_name.get(full).copied())
    }

    /// Inline placed feature, without modifier (added by the code, not by the data).
    pub fn push_inline(&mut self, f: Feature) -> PlacedId {
        self.features.push(f);
        self.feature_names.push(None);
        let feature = FeatureId(self.features.len() as u32 - 1);
        self.placed.push(Placed { feature, placement: Vec::new(), name: None });
        PlacedId(self.placed.len() as u32 - 1)
    }

    pub fn placed_name(&self, id: PlacedId) -> &str {
        self.placed[id.0 as usize].name.as_deref().unwrap_or("<inline>")
    }

    /// Placed feature: registry identifier or inline object.
    pub fn placed_ref(&mut self, j: &Json, c: &mut ParseCtx) -> Result<PlacedId, Error> {
        if let Json::Str(s) = j {
            let full = crate::data::full_id(s);
            if let Some(&id) = self.placed_by_name.get(&full) {
                return Ok(id);
            }
            let pj = c.data.json("placed_feature", &full)?;
            let id = self.parse_placed(&pj, c, Some(full.clone()))?;
            self.placed_by_name.insert(full, id);
            return Ok(id);
        }
        // A bare configured feature (without "placement") equals a placed feature without modifier.
        if j.get("feature").is_none() {
            let f = self.feature_ref(j, c)?;
            self.placed.push(Placed { feature: f, placement: Vec::new(), name: None });
            return Ok(PlacedId(self.placed.len() as u32 - 1));
        }
        self.parse_placed(j, c, None)
    }

    fn parse_placed(&mut self, j: &Json, c: &mut ParseCtx, name: Option<String>) -> Result<PlacedId, Error> {
        let feature = self.feature_ref(j.get("feature").ok_or_else(|| bad("feature"))?, c)?;
        let mut placement = Vec::new();
        for m in j.get("placement").and_then(Json::as_array).unwrap_or(&[]) {
            placement.push(Modifier::parse(m, c)?);
        }
        self.placed.push(Placed { feature, placement, name });
        Ok(PlacedId(self.placed.len() as u32 - 1))
    }

    pub fn feature_ref(&mut self, j: &Json, c: &mut ParseCtx) -> Result<FeatureId, Error> {
        if let Json::Str(s) = j {
            let full = crate::data::full_id(s);
            if let Some(&id) = self.feature_by_name.get(&full) {
                return Ok(id);
            }
            let fj = c.data.json("feature", &full)?;
            let f = self.parse_feature(&fj, c).map_err(|e| bad(format!("{full} : {e}")))?;
            self.features.push(f);
            self.feature_names.push(Some(full.clone()));
            let id = FeatureId(self.features.len() as u32 - 1);
            self.feature_by_name.insert(full, id);
            return Ok(id);
        }
        let f = self.parse_feature(j, c)?;
        self.features.push(f);
        self.feature_names.push(None);
        Ok(FeatureId(self.features.len() as u32 - 1))
    }

    fn placed_list(&mut self, j: Option<&Json>, c: &mut ParseCtx) -> Result<Vec<PlacedId>, Error> {
        let a = j.and_then(Json::as_array).ok_or_else(|| bad("feature list"))?;
        a.iter().map(|x| self.placed_ref(x, c)).collect()
    }

    fn parse_feature(&mut self, j: &Json, c: &mut ParseCtx) -> Result<Feature, Error> {
        let t = ty(j)?;
        let b = c.b;
        let pred = |k: &str, c: &ParseCtx| Pred::parse(j.get(k).ok_or_else(|| bad(k.to_owned()))?, c);
        let pred_or_set = |k: &str, c: &ParseCtx| -> Result<Pred, Error> {
            let v = j.get(k).ok_or_else(|| bad(k.to_owned()))?;
            if v.get("type").is_some() { Pred::parse(v, c) } else { Ok(Pred::Blocks([0; 3], Blockset::parse(v, b)?)) }
        };
        let tag_of = |k: &str| -> Result<u16, Error> {
            let n = j.get(k).and_then(Json::as_str).ok_or_else(|| bad(k.to_owned()))?;
            b.tag(n).ok_or_else(|| Error::Missing(format!("tag {n}")))
        };
        Ok(match t {
            "no_op" => Feature::NoOp,
            "overlay" => Feature::Overlay(self.placed_list(j.get("features"), c)?),
            "sequence" => Feature::Sequence(self.placed_list(j.get("features"), c)?),
            "random_selector" => {
                let mut entries = Vec::new();
                for e in j.get("features").and_then(Json::as_array).ok_or_else(|| bad("features"))? {
                    let p = self.placed_ref(e.get("feature").ok_or_else(|| bad("feature"))?, c)?;
                    entries.push((get_f32(e, "chance")?, p));
                }
                let default = self.placed_ref(j.get("default").ok_or_else(|| bad("default"))?, c)?;
                Feature::RandomSelector { entries, default }
            }
            "simple_random_selector" => Feature::SimpleRandom(self.placed_list(j.get("features"), c)?),
            "random_boolean_selector" => {
                let a = self.placed_ref(j.get("feature_true").ok_or_else(|| bad("feature_true"))?, c)?;
                let f = self.placed_ref(j.get("feature_false").ok_or_else(|| bad("feature_false"))?, c)?;
                Feature::RandomBoolean(a, f)
            }
            "weighted_random_selector" => {
                let mut items = Vec::new();
                for e in j.get("features").and_then(Json::as_array).ok_or_else(|| bad("features"))? {
                    let p = self.placed_ref(e.get("data").ok_or_else(|| bad("data"))?, c)?;
                    items.push((p, get_i32(e, "weight")?));
                }
                Feature::WeightedRandom(Weighted::new(items))
            }
            "simple_block" => Feature::SimpleBlock {
                provider: Provider::parse(j.get("to_place").ok_or_else(|| bad("to_place"))?, c)?,
                schedule_tick: opt_bool(j, "schedule_tick", false),
            },
            "block_column" => {
                let mut layers = Vec::new();
                for l in j.get("layers").and_then(Json::as_array).ok_or_else(|| bad("layers"))? {
                    layers.push(BlockColumnLayer {
                        height: IntP::parse(l.get("height").ok_or_else(|| bad("height"))?)?,
                        provider: Provider::parse(l.get("provider").ok_or_else(|| bad("provider"))?, c)?,
                    });
                }
                Feature::BlockColumn(Box::new(BlockColumn {
                    layers,
                    dir: Dir::from_name(j.get("direction").and_then(Json::as_str).unwrap_or("up"))
                        .ok_or_else(|| bad("direction"))?,
                    allowed: pred("allowed_placement", c)?,
                    prioritize_tip: opt_bool(j, "prioritize_tip", false),
                }))
            }
            "ore" | "scattered_ore" => {
                let mut targets = Vec::new();
                for e in j.get("targets").and_then(Json::as_array).ok_or_else(|| bad("targets"))? {
                    targets.push(OreTarget {
                        rule: Rule::parse(e.get("target").ok_or_else(|| bad("target"))?, b)?,
                        state: b.parse_state(e.get("state").ok_or_else(|| bad("state"))?)?,
                    });
                }
                let o = Box::new(Ore {
                    targets,
                    size: get_i32(j, "size")?,
                    discard: opt_f32(j, "discard_chance_on_air_exposure", 0.0),
                });
                if t == "ore" { Feature::Ore(o) } else { Feature::ScatteredOre(o) }
            }
            "disk" => Feature::Disk(Box::new(Disk {
                provider: Provider::parse(j.get("state_provider").ok_or_else(|| bad("state_provider"))?, c)?,
                target: pred("target", c)?,
                radius: IntP::parse(j.get("radius").ok_or_else(|| bad("radius"))?)?,
                half_height: get_i32(j, "half_height")?,
            })),
            "lake" => Feature::Lake(Box::new(Lake {
                fluid: Provider::parse(j.get("fluid").ok_or_else(|| bad("fluid"))?, c)?,
                barrier: Provider::parse(j.get("barrier").ok_or_else(|| bad("barrier"))?, c)?,
                can_place: j.get("can_place_feature").map_or(Ok(Pred::True), |p| Pred::parse(p, c))?,
                can_replace_air_fluid: j
                    .get("can_replace_with_air_or_fluid")
                    .map_or(Ok(Pred::True), |p| Pred::parse(p, c))?,
                can_replace_barrier: j.get("can_replace_with_barrier").map_or(Ok(Pred::True), |p| Pred::parse(p, c))?,
            })),
            "spring_feature" => {
                let st = j.get("state").ok_or_else(|| bad("state"))?;
                let name =
                    st.get("Name").or_else(|| st.get("id")).and_then(Json::as_str).ok_or_else(|| bad("fluid"))?;
                let block = match name.strip_prefix("minecraft:").unwrap_or(name) {
                    "water" | "flowing_water" => "water",
                    "lava" | "flowing_lava" => "lava",
                    x => return Err(bad(format!("source fluid {x}"))),
                };
                let bid = b.block_id(block).ok_or_else(|| bad("fluid"))?;
                Feature::Spring(Box::new(Spring {
                    state: b.default_state(bid),
                    requires_below: opt_bool(j, "requires_block_below", true),
                    rock_count: opt_i32(j, "rock_count", 4),
                    hole_count: opt_i32(j, "hole_count", 1),
                    valid: Blockset::parse(j.get("valid_blocks").ok_or_else(|| bad("valid_blocks"))?, b)?,
                }))
            }
            "underwater_magma" => Feature::UnderwaterMagma {
                range: get_i32(j, "floor_search_range")?,
                radius: get_i32(j, "placement_radius_around_floor")?,
                prob: get_f32(j, "placement_probability_per_valid_position")?,
            },
            "monster_room" => Feature::MonsterRoom,
            "geode" => Feature::Geode(Box::new(crate::feature_geode::parse(j, c)?)),
            "freeze_top_layer" => Feature::FreezeTopLayer,
            "multiface_growth" => Feature::Multiface(Box::new(Multiface {
                block: {
                    let n = j.get("block").and_then(Json::as_str).unwrap_or("minecraft:glow_lichen");
                    b.block_id(n).ok_or_else(|| Error::Missing(format!("block {n}")))?
                },
                search_range: opt_i32(j, "search_range", 10),
                floor: opt_bool(j, "can_place_on_floor", false),
                ceiling: opt_bool(j, "can_place_on_ceiling", false),
                wall: opt_bool(j, "can_place_on_wall", false),
                chance: opt_f32(j, "chance_of_spreading", 0.5),
                can_be_placed_on: Blockset::parse(
                    j.get("can_be_placed_on").ok_or_else(|| bad("can_be_placed_on"))?,
                    b,
                )?,
            })),
            "vegetation_patch" | "waterlogged_vegetation_patch" => Feature::VegPatch(Box::new(VegPatch {
                replaceable: tag_of("replaceable")?,
                ground: Provider::parse(j.get("ground_state").ok_or_else(|| bad("ground_state"))?, c)?,
                vegetation: self
                    .placed_ref(j.get("vegetation_feature").ok_or_else(|| bad("vegetation_feature"))?, c)?,
                ceiling: j.get("surface").and_then(Json::as_str) == Some("ceiling"),
                depth: IntP::parse(j.get("depth").ok_or_else(|| bad("depth"))?)?,
                extra_bottom: get_f32(j, "extra_bottom_block_chance")?,
                vertical_range: get_i32(j, "vertical_range")?,
                vegetation_chance: get_f32(j, "vegetation_chance")?,
                xz_radius: IntP::parse(j.get("xz_radius").ok_or_else(|| bad("xz_radius"))?)?,
                extra_edge: get_f32(j, "extra_edge_column_chance")?,
                waterlogged: t == "waterlogged_vegetation_patch",
            })),
            "root_system" => Feature::RootSystem(Box::new(RootSystem {
                feature: self.placed_ref(j.get("feature").ok_or_else(|| bad("feature"))?, c)?,
                required_space: get_i32(j, "required_vertical_space_for_tree")?,
                level_test_distance: opt_i32(j, "level_test_distance", 0),
                max_level_deviation: opt_i32(j, "max_level_deviation", 0),
                root_radius: get_i32(j, "root_radius")?,
                root_replaceable: tag_of("root_replaceable")?,
                root_provider: Provider::parse(
                    j.get("root_state_provider").ok_or_else(|| bad("root_state_provider"))?,
                    c,
                )?,
                root_attempts: get_i32(j, "root_placement_attempts")?,
                root_column_max_height: get_i32(j, "root_column_max_height")?,
                hanging_radius: get_i32(j, "hanging_root_radius")?,
                hanging_span: get_i32(j, "hanging_roots_vertical_span")?,
                hanging_provider: Provider::parse(
                    j.get("hanging_root_state_provider").ok_or_else(|| bad("hanging_root_state_provider"))?,
                    c,
                )?,
                hanging_attempts: get_i32(j, "hanging_root_placement_attempts")?,
                allowed_water: get_i32(j, "allowed_vertical_water_for_tree")?,
                allowed_tree_position: pred("allowed_tree_position", c)?,
            })),
            "block_pile" => {
                Feature::BlockPile(Provider::parse(j.get("state_provider").ok_or_else(|| bad("state_provider"))?, c)?)
            }
            "single_block_pillar" => Feature::Pillar(Box::new(Pillar {
                block: Provider::parse(j.get("block").ok_or_else(|| bad("block"))?, c)?,
                can_replace: j.get("can_replace").map_or(Ok(Pred::True), |p| Pred::parse(p, c))?,
                dir: Dir::from_name(j.get("direction").and_then(Json::as_str).unwrap_or("up"))
                    .ok_or_else(|| bad("direction"))?,
                chance: opt_f32(j, "chance_to_continue", 1.0),
                cap: match j.get("cap_feature") {
                    Some(p) => Some(self.placed_ref(p, c)?),
                    None => None,
                },
            })),
            "vines" => Feature::Vines,
            "bamboo" => Feature::Bamboo(get_f32(j, "probability")?),
            "blue_ice" => Feature::BlueIce,
            "iceberg" => Feature::Iceberg(b.parse_state(j.get("state").ok_or_else(|| bad("state"))?)?),
            "spike" => Feature::Spike {
                state: b.parse_state(j.get("state").ok_or_else(|| bad("state"))?)?,
                can_place_on: pred("can_place_on", c)?,
                can_replace: pred("can_replace", c)?,
            },
            "huge_brown_mushroom" | "huge_red_mushroom" => Feature::HugeMushroom(Box::new(HugeMushroom {
                red: t == "huge_red_mushroom",
                cap: Provider::parse(j.get("cap_provider").ok_or_else(|| bad("cap_provider"))?, c)?,
                stem: Provider::parse(j.get("stem_provider").ok_or_else(|| bad("stem_provider"))?, c)?,
                foliage_radius: opt_i32(j, "foliage_radius", 2),
                can_place_on: pred("can_place_on", c)?,
            })),
            "coral_tree" => Feature::CoralTree(self.placed_ref(j.get("feature").ok_or_else(|| bad("feature"))?, c)?),
            "coral_claw" => Feature::CoralClaw(self.placed_ref(j.get("feature").ok_or_else(|| bad("feature"))?, c)?),
            "tree" => Feature::Tree(Box::new(crate::tree::parse_tree(j, c, self)?)),
            "fallen_tree" => Feature::FallenTree(Box::new(crate::tree::parse_fallen(j, c)?)),
            "speleothem" => Feature::Speleothem(Box::new(Speleothem {
                base: b.parse_state(j.get("base_block").ok_or_else(|| bad("base_block"))?)?,
                pointed: b.block(b.parse_state(j.get("pointed_block").ok_or_else(|| bad("pointed_block"))?)?),
                replaceable: pred_or_set("replaceable_blocks", c)?,
                taller: opt_f32(j, "chance_of_taller_generation", 0.2),
                directional: opt_f32(j, "chance_of_directional_spread", 0.7),
                r2: opt_f32(j, "chance_of_spread_radius2", 0.5),
                r3: opt_f32(j, "chance_of_spread_radius3", 0.5),
            })),
            "speleothem_cluster" => Feature::SpeleothemCluster(Box::new(SpeleothemCluster {
                base: b.parse_state(j.get("base_block").ok_or_else(|| bad("base_block"))?)?,
                pointed: b.block(b.parse_state(j.get("pointed_block").ok_or_else(|| bad("pointed_block"))?)?),
                replaceable: pred_or_set("replaceable_blocks", c)?,
                search_range: get_i32(j, "floor_to_ceiling_search_range")?,
                height: IntP::parse(j.get("height").ok_or_else(|| bad("height"))?)?,
                radius: IntP::parse(j.get("radius").ok_or_else(|| bad("radius"))?)?,
                max_diff: get_i32(j, "max_stalagmite_stalactite_height_diff")?,
                height_deviation: get_i32(j, "height_deviation")?,
                layer_thickness: IntP::parse(
                    j.get("speleothem_block_layer_thickness").ok_or_else(|| bad("speleothem_block_layer_thickness"))?,
                )?,
                density: FloatP::parse(j.get("density").ok_or_else(|| bad("density"))?)?,
                wetness: FloatP::parse(j.get("wetness").ok_or_else(|| bad("wetness"))?)?,
                chance_at_max: get_f32(j, "chance_of_speleothem_at_max_distance_from_center")?,
                max_edge: get_i32(j, "max_distance_from_edge_affecting_chance_of_speleothem")?,
                max_center: get_i32(j, "max_distance_from_center_affecting_height_bias")?,
            })),
            "large_dripstone" => Feature::LargeDripstone(Box::new(LargeDripstone {
                replaceable: pred_or_set("replaceable_blocks", c)?,
                search_range: opt_i32(j, "floor_to_ceiling_search_range", 30),
                column_radius: IntP::parse(j.get("column_radius").ok_or_else(|| bad("column_radius"))?)?,
                height_scale: FloatP::parse(j.get("height_scale").ok_or_else(|| bad("height_scale"))?)?,
                ratio: get_f32(j, "max_column_radius_to_cave_height_ratio")?,
                stalactite_bluntness: FloatP::parse(
                    j.get("stalactite_bluntness").ok_or_else(|| bad("stalactite_bluntness"))?,
                )?,
                stalagmite_bluntness: FloatP::parse(
                    j.get("stalagmite_bluntness").ok_or_else(|| bad("stalagmite_bluntness"))?,
                )?,
                wind_speed: FloatP::parse(j.get("wind_speed").ok_or_else(|| bad("wind_speed"))?)?,
                min_radius_for_wind: get_i32(j, "min_radius_for_wind")?,
                min_bluntness_for_wind: get_f32(j, "min_bluntness_for_wind")?,
            })),
            "block_blob" => Feature::BlockBlob {
                state: b.parse_state(j.get("state").ok_or_else(|| bad("state"))?)?,
                can_place_on: pred("can_place_on", c)?,
            },
            "sculk_patch" => Feature::SculkPatch(SculkPatch {
                charge_count: get_i32(j, "charge_count")?,
                amount_per_charge: get_i32(j, "amount_per_charge")?,
                spread_attempts: get_i32(j, "spread_attempts")?,
                growth_rounds: get_i32(j, "growth_rounds")?,
                spread_rounds: get_i32(j, "spread_rounds")?,
            }),
            "delta_feature" => Feature::Delta(Box::new(crate::feature_nether::parse_delta(j, c)?)),
            "huge_fungus" => Feature::HugeFungus(Box::new(crate::feature_nether::parse_fungus(j, c)?)),
            "projected_random_patchy_square" => {
                Feature::PatchySquare(Box::new(crate::feature_nether::parse_patchy(j, c)?))
            }
            "random_neighbor_spread" => Feature::NeighborSpread(Box::new(crate::feature_nether::parse_spread(j, c)?)),
            "netherrack_replace_blobs" => Feature::ReplaceBlobs(Box::new(crate::feature_nether::parse_blobs(j, c)?)),
            "stepped_column_cluster" => Feature::SteppedColumns(Box::new(crate::feature_nether::parse_columns(j, c)?)),
            "end_spike" => Feature::EndSpike(crate::feature_end::parse_spikes(j)?),
            "end_gateway" => Feature::EndGateway {
                exit: match j.get("exit").and_then(Json::as_array) {
                    Some([x, y, z]) => Some([
                        x.as_i32().ok_or_else(|| bad("exit"))?,
                        y.as_i32().ok_or_else(|| bad("exit"))?,
                        z.as_i32().ok_or_else(|| bad("exit"))?,
                    ]),
                    Some(_) => return Err(bad("exit")),
                    None => None,
                },
                exact: opt_bool(j, "exact", false),
            },
            "end_island" => Feature::EndIsland,
            "end_platform" => Feature::EndPlatform,
            "chorus_plant" => Feature::ChorusPlant,
            "template" => Feature::Template(Box::new(crate::feature_templ::TemplateCfg::parse(j)?)),
            "fossil" => Feature::Fossil(Box::new(crate::feature_templ::FossilCfg::parse(j)?)),
            "fill_layer" => Feature::FillLayer {
                height: get_i32(j, "height")?,
                state: b.parse_state(j.get("state").ok_or_else(|| bad("state"))?)?,
            },
            "replace_single_block" => {
                let mut targets = Vec::new();
                for e in j.get("targets").and_then(Json::as_array).ok_or_else(|| bad("targets"))? {
                    targets.push(OreTarget {
                        rule: Rule::parse(e.get("target").ok_or_else(|| bad("target"))?, b)?,
                        state: b.parse_state(e.get("state").ok_or_else(|| bad("state"))?)?,
                    });
                }
                Feature::ReplaceBlock(targets)
            }
            "void_start_platform" => Feature::VoidStartPlatform,
            "bonus_chest" => Feature::BonusChest,
            other => Feature::Unsupported(other.to_owned()),
        })
    }
}

/// Places the configured feature `id` at `p`.
pub fn place_feature(cx: &mut Ctx, id: FeatureId, rng: &mut DecoRng, p: [i32; 3]) -> bool {
    let d = cx.d;
    let f = &d.reg.features[id.0 as usize];
    match f {
        Feature::NoOp => true,
        Feature::Overlay(list) => {
            let mut any = false;
            for &pl in list {
                any |= crate::placement::place(cx, pl, rng, p, None);
            }
            any
        }
        Feature::Sequence(list) => {
            for &pl in list {
                if !crate::placement::place(cx, pl, rng, p, None) {
                    return false;
                }
            }
            true
        }
        Feature::RandomSelector { entries, default } => {
            for &(chance, pl) in entries {
                if rng.next_float() < chance {
                    return crate::placement::place(cx, pl, rng, p, None);
                }
            }
            crate::placement::place(cx, *default, rng, p, None)
        }
        Feature::SimpleRandom(list) => {
            let i = rng.next_int_bounded(list.len() as i32) as usize;
            crate::placement::place(cx, list[i], rng, p, None)
        }
        Feature::RandomBoolean(a, b) => {
            let pick = if rng.next_bool() { *a } else { *b };
            crate::placement::place(cx, pick, rng, p, None)
        }
        Feature::WeightedRandom(w) => match w.pick(rng) {
            Some(&pl) => crate::placement::place(cx, pl, rng, p, None),
            None => false,
        },
        Feature::SimpleBlock { provider, schedule_tick } => {
            crate::feature_basic::simple_block(cx, provider, *schedule_tick, rng, p)
        }
        Feature::BlockColumn(c) => crate::feature_basic::block_column(cx, c, rng, p),
        Feature::Ore(o) => crate::feature_basic::ore(cx, o, rng, p),
        Feature::ScatteredOre(o) => crate::feature_basic::scattered_ore(cx, o, rng, p),
        Feature::Disk(c) => crate::feature_basic::disk(cx, c, rng, p),
        Feature::Lake(c) => crate::feature_basic::lake(cx, c, rng, p),
        Feature::Spring(c) => crate::feature_basic::spring(cx, c, p),
        Feature::UnderwaterMagma { range, radius, prob } => {
            crate::feature_basic::underwater_magma(cx, *range, *radius, *prob, rng, p)
        }
        Feature::MonsterRoom => crate::feature_basic::monster_room(cx, rng, p),
        Feature::Geode(g) => crate::feature_geode::place(cx, g, rng, p),
        Feature::FreezeTopLayer => crate::feature_basic::freeze_top_layer(cx, p),
        Feature::Multiface(m) => crate::feature_veg::multiface(cx, m, rng, p),
        Feature::VegPatch(v) => crate::feature_veg::vegetation_patch(cx, v, rng, p),
        Feature::RootSystem(v) => crate::feature_veg::root_system(cx, v, rng, p),
        Feature::BlockPile(pr) => crate::feature_nether::block_pile(cx, pr, rng, p),
        Feature::Pillar(pl) => crate::feature_nether::pillar(cx, pl, rng, p),
        Feature::Delta(c) => crate::feature_nether::delta(cx, c, rng, p),
        Feature::HugeFungus(c) => crate::feature_nether::huge_fungus(cx, c, rng, p),
        Feature::PatchySquare(c) => crate::feature_nether::patchy_square(cx, c, rng, p),
        Feature::NeighborSpread(c) => crate::feature_nether::neighbor_spread(cx, c, rng, p),
        Feature::ReplaceBlobs(c) => crate::feature_nether::replace_blobs(cx, c, rng, p),
        Feature::SteppedColumns(c) => crate::feature_nether::stepped_columns(cx, c, rng, p),
        Feature::EndSpike(s) => crate::feature_end::end_spike(cx, s, rng, p),
        Feature::EndGateway { exit, exact } => crate::feature_end::end_gateway(cx, p, *exit, *exact),
        Feature::EndIsland => crate::feature_end::end_island(cx, rng, p),
        Feature::EndPlatform => crate::feature_end::end_platform(cx, p),
        Feature::ChorusPlant => crate::feature_end::chorus_plant(cx, rng, p),
        Feature::Vines => crate::feature_veg::vines(cx, p),
        Feature::Bamboo(prob) => crate::feature_veg::bamboo(cx, *prob, rng, p),
        Feature::BlueIce => crate::feature_ice::blue_ice(cx, rng, p),
        Feature::Iceberg(s) => crate::feature_ice::iceberg(cx, *s, rng, p),
        Feature::Spike { state, can_place_on, can_replace } => {
            crate::feature_ice::spike(cx, *state, can_place_on, can_replace, rng, p)
        }
        Feature::HugeMushroom(m) => crate::feature_veg::huge_mushroom(cx, m, rng, p),
        Feature::CoralTree(pl) => crate::feature_veg::coral_tree(cx, *pl, rng, p),
        Feature::CoralClaw(pl) => crate::feature_veg::coral_claw(cx, *pl, rng, p),
        Feature::Tree(t) => crate::tree::place_tree(cx, t, rng, p),
        Feature::FallenTree(t) => crate::tree::place_fallen(cx, t, rng, p),
        Feature::Speleothem(s) => crate::feature_cave::speleothem(cx, s, rng, p),
        Feature::SpeleothemCluster(s) => crate::feature_cave::speleothem_cluster(cx, s, rng, p),
        Feature::LargeDripstone(s) => crate::feature_cave::large_dripstone(cx, s, rng, p),
        Feature::BlockBlob { state, can_place_on } => crate::feature_veg::block_blob(cx, *state, can_place_on, rng, p),
        Feature::SculkPatch(c) => crate::feature_sculk::sculk_patch(cx, c, rng, p),
        Feature::Template(t) => crate::feature_templ::place_template_feature(cx, t, rng, p),
        Feature::Fossil(f) => crate::feature_templ::place_fossil(cx, f, rng, p),
        Feature::FillLayer { height, state } => crate::feature_basic::fill_layer(cx, *height, *state, p),
        Feature::ReplaceBlock(t) => crate::feature_basic::replace_single_block(cx, t, rng, p),
        Feature::VoidStartPlatform => crate::feature_basic::void_start_platform(cx, p),
        // Placed by the server at world creation (`rc-sim`), never by a step.
        Feature::BonusChest => false,
        Feature::Unsupported(name) => {
            cx.note_missing(name);
            false
        }
    }
}
