//! Material rules (spec generation-terrain § 4–5): reading of the `material_rule` and
//! `material_condition` registries, evaluation at the block (surface, bedrock, deepslate, ore veins),
//! surface walk of a chunk with the extensions of the eroded badlands and the icebergs, terracotta
//! bands, and the "top material" replayed by the carved caves.

use std::cell::RefCell;

use hashbrown::HashMap;

use crate::biome::{BiomeSource, ChunkBiomes, Searcher, zoom_cached};
use crate::data::{DataPack, full_id};
use crate::density::Axis;
use crate::json::Json;
use crate::math::{floor_f, round_half_up};
use crate::noise::Simplex;
use crate::palette::{BlockId, BlockTable};
use crate::proto::ProtoTerrain;
use crate::rng::{Positional, Rng};
use crate::sampler::{Ctx, Program, S, Sid};
use crate::volume::Volume;
use crate::{Error, Generator};

pub type RuleId = u32;
pub type CondId = u32;

/// Root specialised for a band where the rule surely returns "nothing".
const NO_RULE: RuleId = RuleId::MAX;

/// Paths of the material rules (switches of `crate::opt`, frozen at first use); the
/// equivalence tests set their own in [`MaterialSystem::opts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatOpts {
    /// `ORE_IDX`: index of the block in the vein volume drawn from a column base.
    pub ore_idx: bool,
    /// `ORE_EARLY`: vein skipped on a y row where its density is surely a constant ≤ 0.
    pub ore_early: bool,
    /// `SURF_SKIP`: root not evaluated on the rows where it surely returns "nothing".
    pub surf_skip: bool,
    /// `SURF_BANDS`: tree specialised per y band.
    pub surf_bands: bool,
    /// `VEIN_YRANGE`: vein pre-filling restricted to the live rows.
    pub vein_yrange: bool,
    /// `VEIN_LAZYRICH`: richness only on the rows where a density is positive.
    pub vein_lazyrich: bool,
    /// `VEIN_SHARED`: a single y range for all the veins (with `vein_yrange`).
    pub vein_shared: bool,
    /// `MAT_REUSE`: state reused per thread, veins of the cave material read on demand.
    pub reuse: bool,
    /// `MAT_COLCACHE`: column constants kept, cold temperature discarded by bound.
    pub colcache: bool,
}

impl MatOpts {
    /// Code from before the mission.
    pub const OLD: MatOpts = MatOpts {
        ore_idx: false,
        ore_early: false,
        surf_skip: false,
        surf_bands: false,
        vein_yrange: false,
        vein_lazyrich: false,
        vein_shared: false,
        reuse: false,
        colcache: false,
    };
    pub const NEW: MatOpts = MatOpts {
        ore_idx: true,
        ore_early: true,
        surf_skip: true,
        surf_bands: true,
        vein_yrange: true,
        vein_lazyrich: true,
        vein_shared: true,
        reuse: true,
        colcache: true,
    };

    pub fn from_env() -> MatOpts {
        use crate::opt;
        MatOpts {
            ore_idx: opt::ore_idx(),
            ore_early: opt::ore_early(),
            surf_skip: opt::surf_skip(),
            surf_bands: opt::surf_bands(),
            vein_yrange: opt::vein_yrange(),
            vein_lazyrich: opt::vein_lazyrich(),
            vein_shared: opt::vein_shared(),
            reuse: opt::mat_reuse(),
            colcache: opt::mat_colcache(),
        }
    }
}

/// Proven bound of |temperature noise| (2D simplex): 3 corners, each at most
/// 70 · √2 · max((0.5 − r²)⁴ · r) ≈ 0.91, i.e. < 2.75 (the measurement in test reaches 0.998). The
/// altitude correction therefore cannot remove more than `(8 · bound + y − snow) · 0.05 / 40`.
pub const TEMPERATURE_NOISE_BOUND: f32 = 2.75;

/// Density threshold of a vein decided by y alone: outside `[min, max[` of `input` (function of y
/// alone), the density is a constant ≤ 0 and the vein returns "nothing" without a draw.
#[derive(Debug, Clone, Copy)]
struct YGate {
    input: Sid,
    min: f32,
    max: f32,
    outside: f32,
}

/// Node that depends only on y (and reads no cache): its point value at (0, y, 0) is
/// that of the volume in any column.
fn y_only(p: &Program, s: Sid) -> bool {
    matches!(
        p.nodes[s as usize],
        S::Const(_)
            | S::GradClamp { axis: Axis::Y, .. }
            | S::GradRepeat { axis: Axis::Y, .. }
            | S::GradMirror { axis: Axis::Y, .. }
    )
}

/// Threshold of the density `s` of a vein, if its root is a choice on a function of y whose
/// "out of range" branch is a constant ≤ 0 (checked on the compiled tree, nothing assumed).
fn gate_of(p: &Program, s: Sid) -> Option<YGate> {
    let (input, min, max, outside) = match &p.nodes[s as usize] {
        S::RangeConst { input, min, max, outside, .. } => (*input, *min, *max, *outside),
        S::Range { input, min, max, outside, .. } => match p.nodes[*outside as usize] {
            S::Const(c) => (*input, *min, *max, c),
            _ => return None,
        },
        _ => return None,
    };
    (outside <= 0.0 && y_only(p, input)).then_some(YGate { input, min, max, outside })
}

/// Facts known on a y band: dead veins, conditions decided by y, biomes of the chunk,
/// and (hypothesis) block under the preliminary surface.
struct Facts<'a> {
    y: i32,
    dead: &'a [bool],
    biome_static: Option<&'a [u8]>,
    below_surface: bool,
}

/// Safe value of a condition under `f`, `None` if it depends on the block.
fn cond_fact(conds: &[Cond], c: CondId, f: &Facts) -> Option<bool> {
    match &conds[c as usize] {
        Cond::Biome { slot, .. } => match f.biome_static.map(|b| b[*slot as usize]) {
            Some(1) => Some(true),
            Some(2) => Some(false),
            _ => None,
        },
        Cond::VerticalGradient { t, f: fa, .. } => {
            if f.y <= *t {
                Some(true)
            } else if f.y >= *fa {
                Some(false)
            } else {
                None
            }
        }
        Cond::YAbove { anchor, mult: 0, add: false } => Some(f.y >= *anchor),
        Cond::Not(i) => cond_fact(conds, *i, f).map(|b| !b),
        Cond::AbovePreliminarySurface if f.below_surface => Some(false),
        _ => None,
    }
}

/// Does the rule `r` surely return "nothing" under `f`?
fn surely_none(rules: &[Rule], conds: &[Cond], r: RuleId, f: &Facts) -> bool {
    match &rules[r as usize] {
        Rule::Block(_) | Rule::Bandlands => false,
        Rule::Ore(i) => f.dead[*i as usize],
        Rule::Condition(c, t) => match cond_fact(conds, *c, f) {
            Some(false) => true,
            _ => surely_none(rules, conds, *t, f),
        },
        Rule::Sequence(v) => v.iter().all(|c| surely_none(rules, conds, *c, f)),
    }
}

/// Does the rule `r` surely return a block (tree already specialised)?
fn surely_some(rules: &[Rule], r: RuleId) -> bool {
    match &rules[r as usize] {
        Rule::Block(_) | Rule::Bandlands => true,
        Rule::Sequence(v) => v.iter().any(|c| surely_some(rules, *c)),
        Rule::Condition(..) | Rule::Ore(_) => false,
    }
}

/// Copy of `r` where the conditions decided by `f` are removed (`None`: surely "nothing").
/// An unchanged rule keeps its identifier; the new ones are added to `rules`.
fn specialize(
    rules: &mut Vec<Rule>,
    conds: &[Cond],
    r: RuleId,
    f: &Facts,
    memo: &mut HashMap<RuleId, Option<RuleId>>,
) -> Option<RuleId> {
    if let Some(m) = memo.get(&r) {
        return *m;
    }
    let out = match rules[r as usize].clone() {
        Rule::Block(_) | Rule::Bandlands => Some(r),
        Rule::Ore(i) => (!f.dead[i as usize]).then_some(r),
        Rule::Condition(c, t) => match cond_fact(conds, c, f) {
            Some(false) => None,
            Some(true) => specialize(rules, conds, t, f, memo),
            None => match specialize(rules, conds, t, f, memo) {
                None => None,
                Some(t2) if t2 == t => Some(r),
                Some(t2) => {
                    rules.push(Rule::Condition(c, t2));
                    Some((rules.len() - 1) as RuleId)
                }
            },
        },
        Rule::Sequence(v) => {
            let mut kept = Vec::with_capacity(v.len());
            for c in &v {
                if let Some(s) = specialize(rules, conds, *c, f, memo) {
                    kept.push(s);
                    if surely_some(rules, s) {
                        break;
                    }
                }
            }
            match kept.len() {
                0 => None,
                1 => Some(kept[0]),
                _ if kept == v => Some(r),
                _ => {
                    rules.push(Rule::Sequence(kept));
                    Some((rules.len() - 1) as RuleId)
                }
            }
        }
    };
    memo.insert(r, out);
    out
}

/// Vertical generation context (anchors): `min_gen_y`, `gen_depth`, sea.
#[derive(Debug, Clone, Copy)]
pub struct GenContext {
    pub min_y: i32,
    pub depth: i32,
    pub sea: i32,
}

impl GenContext {
    pub fn anchor(&self, j: &Json) -> Result<i32, Error> {
        let get = |k: &str| j.get(k).and_then(Json::as_i32);
        if let Some(v) = get("absolute") {
            Ok(v)
        } else if let Some(o) = get("above_bottom") {
            Ok(self.min_y + o)
        } else if let Some(o) = get("below_top") {
            Ok(self.depth - 1 + self.min_y - o)
        } else if let Some(o) = get("relative_to_sea_level") {
            Ok(self.sea + o)
        } else {
            Err(Error::Format("vertical anchor".into()))
        }
    }
}

#[derive(Debug, Clone)]
pub enum Cond {
    /// Set of biomes (flag per biome identifier) and shortcut cell per chunk.
    Biome {
        set: Vec<bool>,
        slot: u32,
    },
    Noise {
        slot: u32,
        min: f64,
        max: f64,
    },
    VerticalGradient {
        t: i32,
        f: i32,
        random: Positional,
    },
    YAbove {
        anchor: i32,
        mult: i32,
        add: bool,
    },
    Water {
        offset: i32,
        mult: i32,
        add: bool,
    },
    Temperature,
    Steep,
    Not(CondId),
    Hole,
    AbovePreliminarySurface,
    StoneDepth {
        offset: i32,
        add_sd: bool,
        range: i32,
        ceiling: bool,
    },
}

#[derive(Debug, Clone)]
pub enum Rule {
    Block(BlockId),
    Sequence(Vec<RuleId>),
    Condition(CondId, RuleId),
    Bandlands,
    Ore(u32),
}

#[derive(Debug, Clone)]
pub struct OreRule {
    pub ore: BlockId,
    pub raw: BlockId,
    pub filler: BlockId,
    pub raw_chance: f32,
    pub density: Sid,
    pub richness: Sid,
    pub gap: Sid,
}

/// Climate of a biome useful here (base temperature, "frozen" modifier).
#[derive(Debug, Clone, Copy, Default)]
pub struct BiomeClimate {
    pub temperature: f32,
    pub frozen: bool,
}

/// Temperature noises of the biomes (fixed seeds 1234, 3456, 2345).
#[derive(Debug, Clone)]
struct TemperatureNoises {
    temperature: Simplex,
    frozen: [Simplex; 3],
    info: Simplex,
}

impl TemperatureNoises {
    fn new() -> Self {
        let temperature = Simplex::new_without_offset(&mut Rng::legacy(1234));
        let mut r = Rng::legacy(3456);
        let frozen = [
            Simplex::new_without_offset(&mut r),
            Simplex::new_without_offset(&mut r),
            Simplex::new_without_offset(&mut r),
        ];
        let info = Simplex::new_without_offset(&mut Rng::legacy(2345));
        TemperatureNoises { temperature, frozen, info }
    }

    fn frozen_stack(&self, x: f64, z: f64) -> f32 {
        let mut v = 0.0f32;
        for (n, (freq, amp)) in
            self.frozen.iter().zip([(1.0, 0.142_857_15f32), (0.5, 0.285_714_3), (0.25, 0.571_428_6)])
        {
            v += amp * n.get2(x * freq, z * freq);
        }
        v
    }
}

/// Material system of a generator (prepared once).
#[derive(Debug)]
pub struct MaterialSystem {
    pub rules: Vec<Rule>,
    pub conds: Vec<Cond>,
    pub ores: Vec<OreRule>,
    pub root: RuleId,
    /// Pre-filling order of the veins at compilation of the root rule.
    prefill: Vec<u32>,
    /// Noise (program index) of each noise-condition cache slot, and whether it is 3D.
    noise_slots: Vec<(u32, bool)>,
    biome_slots: u32,
    surface: u32,
    secondary: u32,
    clay_offset: u32,
    badlands: [u32; 3],
    iceberg: [u32; 3],
    random: Positional,
    ore_random: Positional,
    bands: [BlockId; 192],
    pub default_block: BlockId,
    sea: i32,
    chunk_surface: Sid,
    snow_block: BlockId,
    packed_ice: BlockId,
    eroded_badlands: Option<u16>,
    frozen_oceans: [Option<u16>; 2],
    climates: Vec<BiomeClimate>,
    temps: TemperatureNoises,
    /// Paths chosen (switches at startup; the tests set their own).
    pub opts: MatOpts,
    /// y threshold of the density of each vein, if proven.
    gates: Vec<Option<YGate>>,
    /// Bottom of the generation height and number of rows of the per-y tables.
    y_min: i32,
    depth: i32,
    /// Per y row (from `y_min`) then per vein: density surely constant ≤ 0.
    dead: Vec<bool>,
    /// y bands with constant facts: `(start, end excluded, specialised root or NO_RULE)`.
    y_bands: Vec<(i32, i32, RuleId)>,
    /// Band of each y row (from `y_min`).
    band_of: Vec<u16>,
}

/// Class of a y row for the root skip (`MatState::skip`).
const LIVE: u8 = 0;
const DEAD: u8 = 1;
/// Surely "nothing" under the preliminary surface (once it is known for the column).
const DEAD_BELOW: u8 = 2;

struct Builder<'a> {
    pack: &'a DataPack,
    g: &'a mut Generator,
    blocks: &'a mut BlockTable,
    biomes: &'a HashMap<String, u16>,
    n_biomes: usize,
    gc: GenContext,
    rules: Vec<Rule>,
    conds: Vec<Cond>,
    ores: Vec<OreRule>,
    named_rules: HashMap<String, RuleId>,
    named_conds: HashMap<String, CondId>,
    noise_slots: Vec<(u32, bool)>,
    biome_slots: u32,
}

fn bad(w: &str) -> Error {
    Error::Format(format!("material rule: {w}"))
}

fn ty(j: &Json) -> &str {
    let t = j.get("type").and_then(Json::as_str).unwrap_or_default();
    t.strip_prefix("minecraft:").unwrap_or(t)
}

impl Builder<'_> {
    fn rule(&mut self, j: &Json) -> Result<RuleId, Error> {
        if let Some(name) = j.as_str() {
            let id = full_id(name);
            if let Some(r) = self.named_rules.get(&id) {
                return Ok(*r);
            }
            let inner = self.pack.json("material_rule", &id)?;
            let r = self.rule(&inner)?;
            self.named_rules.insert(id, r);
            return Ok(r);
        }
        let r = match ty(j) {
            "block" => {
                let s = j.get("result_state").ok_or_else(|| bad("result_state"))?;
                Rule::Block(self.state(s)?)
            }
            "sequence" => {
                let list = j.get("sequence").and_then(Json::as_array).ok_or_else(|| bad("sequence"))?;
                let v = list.iter().map(|r| self.rule(r)).collect::<Result<Vec<_>, _>>()?;
                if v.len() == 1 {
                    return Ok(v[0]);
                }
                Rule::Sequence(v)
            }
            "condition" => {
                let c = self.cond(j.get("if_true").ok_or_else(|| bad("if_true"))?)?;
                let r = self.rule(j.get("then_run").ok_or_else(|| bad("then_run"))?)?;
                Rule::Condition(c, r)
            }
            "bandlands" => Rule::Bandlands,
            "ore_vein" => {
                let st = |b: &mut Self, k: &str| b.state(j.get(k).ok_or_else(|| bad(k))?);
                let f = |b: &mut Self, k: &str| -> Result<Sid, Error> {
                    let d = crate::density::parse(j.get(k).ok_or_else(|| bad(k))?)?;
                    b.g.compile_function(&d)
                };
                let ore = OreRule {
                    ore: st(self, "ore_block")?,
                    raw: st(self, "raw_ore_block")?,
                    filler: st(self, "filler_block")?,
                    raw_chance: j.get("raw_ore_chance").and_then(Json::as_f32).ok_or_else(|| bad("raw_ore_chance"))?,
                    density: f(self, "density")?,
                    richness: f(self, "richness")?,
                    gap: f(self, "filler_gap")?,
                };
                self.ores.push(ore);
                Rule::Ore((self.ores.len() - 1) as u32)
            }
            other => return Err(bad(&format!("type {other}"))),
        };
        self.rules.push(r);
        Ok((self.rules.len() - 1) as RuleId)
    }

    fn state(&mut self, j: &Json) -> Result<BlockId, Error> {
        if let Some(s) = j.as_str() {
            return Ok(self.blocks.intern(s));
        }
        let name = j.get("Name").and_then(Json::as_str).ok_or_else(|| bad("state"))?;
        let mut s = full_id(name);
        if let Some(Json::Obj(p)) = j.get("Properties") {
            let props: Vec<String> = p.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default())).collect();
            s = format!("{s}[{}]", props.join(","));
        }
        Ok(self.blocks.intern(&s))
    }

    fn cond(&mut self, j: &Json) -> Result<CondId, Error> {
        if let Some(name) = j.as_str() {
            let id = full_id(name);
            if let Some(c) = self.named_conds.get(&id) {
                return Ok(*c);
            }
            let inner = self.pack.json("material_condition", &id)?;
            let c = self.cond(&inner)?;
            self.named_conds.insert(id, c);
            return Ok(c);
        }
        let int = |k: &str| j.get(k).and_then(Json::as_i32).ok_or_else(|| bad(k));
        let boolean = |k: &str| j.get(k).and_then(Json::as_bool).unwrap_or(false);
        let c = match ty(j) {
            "biome" => {
                let mut set = vec![false; self.n_biomes];
                let b = j.get("biome_is").ok_or_else(|| bad("biome_is"))?;
                let names: Vec<&str> = match b {
                    Json::Str(s) => vec![s.as_str()],
                    Json::Arr(a) => a.iter().filter_map(Json::as_str).collect(),
                    _ => return Err(bad("biome_is")),
                };
                for n in names {
                    if let Some(i) = crate::data::with_full_id(n, |f| self.biomes.get(f)) {
                        set[*i as usize] = true;
                    }
                }
                self.biome_slots += 1;
                Cond::Biome { set, slot: self.biome_slots - 1 }
            }
            "noise_threshold" => {
                let noise = j.get("noise").and_then(Json::as_str).ok_or_else(|| bad("noise"))?;
                let n = self.g.noise(&full_id(noise))?;
                let is3d = boolean("is_3d");
                let slot = match self.noise_slots.iter().position(|s| *s == (n, is3d)) {
                    Some(p) => p,
                    None => {
                        self.noise_slots.push((n, is3d));
                        self.noise_slots.len() - 1
                    }
                } as u32;
                Cond::Noise {
                    slot,
                    min: j.get("min_threshold").and_then(Json::as_f64).ok_or_else(|| bad("min_threshold"))?,
                    max: j.get("max_threshold").and_then(Json::as_f64).ok_or_else(|| bad("max_threshold"))?,
                }
            }
            "vertical_gradient" => {
                let name = j.get("random_name").and_then(Json::as_str).ok_or_else(|| bad("random_name"))?;
                Cond::VerticalGradient {
                    t: self.gc.anchor(j.get("true_at_and_below").ok_or_else(|| bad("true_at_and_below"))?)?,
                    f: self.gc.anchor(j.get("false_at_and_above").ok_or_else(|| bad("false_at_and_above"))?)?,
                    random: self.g.positional().from_hash_of(&full_id(name)).fork_positional(),
                }
            }
            "y_above" => Cond::YAbove {
                anchor: self.gc.anchor(j.get("anchor").ok_or_else(|| bad("anchor"))?)?,
                mult: int("surface_depth_multiplier")?,
                add: boolean("add_stone_depth"),
            },
            "water" => Cond::Water {
                offset: int("offset")?,
                mult: int("surface_depth_multiplier")?,
                add: boolean("add_stone_depth"),
            },
            "temperature" => Cond::Temperature,
            "steep" => Cond::Steep,
            "not" => Cond::Not(self.cond(j.get("invert").ok_or_else(|| bad("invert"))?)?),
            "hole" => Cond::Hole,
            "above_preliminary_surface" => Cond::AbovePreliminarySurface,
            "stone_depth" => Cond::StoneDepth {
                offset: int("offset")?,
                add_sd: boolean("add_surface_depth"),
                range: int("secondary_depth_range")?,
                ceiling: j.get("surface_type").and_then(Json::as_str) == Some("ceiling"),
            },
            other => return Err(bad(&format!("condition {other}"))),
        };
        self.conds.push(c);
        Ok((self.conds.len() - 1) as CondId)
    }
}

/// Compilation order (depth first) of the veins of the rule `r`, without duplicates.
fn prefill_order(rules: &[Rule], r: RuleId, out: &mut Vec<u32>) {
    match &rules[r as usize] {
        Rule::Sequence(v) => v.iter().for_each(|c| prefill_order(rules, *c, out)),
        Rule::Condition(_, t) => prefill_order(rules, *t, out),
        Rule::Ore(i) => {
            if !out.contains(i) {
                out.push(*i);
            }
        }
        Rule::Block(_) | Rule::Bandlands => {}
    }
}

fn generate_bands(r: &mut Rng, t: &mut BlockTable) -> [BlockId; 192] {
    let mut b = [t.intern("minecraft:terracotta"); 192];
    let orange = t.intern("minecraft:orange_terracotta");
    let mut i = 0usize;
    while i < 192 {
        i += r.next_int_bounded(5) as usize + 1;
        if i < 192 {
            b[i] = orange;
        }
        i += 1;
    }
    for (w, name) in [(1, "yellow"), (2, "brown"), (1, "red")] {
        let s = rc_data::ids::with_key(&["minecraft:", name, "_terracotta"], |k| t.intern(k));
        let n = r.next_int_bounded(10) + 6;
        for _ in 0..n {
            let width = w + r.next_int_bounded(3);
            let start = r.next_int_bounded(192);
            let mut p = 0;
            while start + p < 192 && p < width {
                b[(start + p) as usize] = s;
                p += 1;
            }
        }
    }
    let white = t.intern("minecraft:white_terracotta");
    let light = t.intern("minecraft:light_gray_terracotta");
    let n = r.next_int_bounded(7) + 9;
    let (mut k, mut start) = (0, 0i32);
    while k < n && start < 192 {
        b[start as usize] = white;
        if start - 1 > 0 && r.next_bool() {
            b[(start - 1) as usize] = light;
        }
        if start + 1 < 192 && r.next_bool() {
            b[(start + 1) as usize] = light;
        }
        k += 1;
        start += r.next_int_bounded(16) + 4;
    }
    b
}

/// Source of the biomes seen by the conditions: biomes of the 3 × 3 chunks (zoom blur, y clamped to the
/// chunk height) or direct biome source without cache (carved caves).
pub enum BiomeView<'a> {
    Region { chunks: [&'a ChunkBiomes; 9], cx: i32, cz: i32, zoom_seed: i64, min_qy: i32, max_qy: i32 },
    Source { src: &'a BiomeSource, zoom_seed: i64, searcher: Searcher },
}

impl BiomeView<'_> {
    pub fn get(&mut self, g: &Generator, x: i32, y: i32, z: i32) -> u16 {
        match self {
            BiomeView::Region { chunks, cx, cz, zoom_seed, min_qy, max_qy } => {
                let (qx, qy, qz) = zoom_cached(*zoom_seed, x, y, z);
                let c = chunks[(((qx >> 2) - *cx + 1) * 3 + (qz >> 2) - *cz + 1) as usize];
                let qy = qy.clamp(*min_qy, *max_qy);
                c.get(qx & 3, qy - c.min_qy, qz & 3)
            }
            BiomeView::Source { src, zoom_seed, searcher } => {
                let (qx, qy, qz) = zoom_cached(*zoom_seed, x, y, z);
                src.biome_at(g, &mut Ctx::uncached(), searcher, qx, qy, qz)
            }
        }
    }
}

/// Evaluation state (game rule context): column, block, lazy caches.
pub struct MatState {
    x: i32,
    z: i32,
    grad_x: i32,
    grad_z: i32,
    sd: i32,
    secondary: Option<f64>,
    min_surface: Option<i32>,
    y: i32,
    water: i32,
    below: i32,
    above: i32,
    biome: Option<u16>,
    xz_stamp: u64,
    y_stamp: u64,
    noise_val: Vec<f64>,
    noise_stamp: Vec<u64>,
    /// Shortcut of the biome conditions: 0 to test, 1 always true, 2 always false.
    biome_static: Vec<u8>,
    ore_vol: Volume,
    ore_bufs: Vec<Option<(Vec<f32>, Vec<f32>)>>,
    prelim_vol: Volume,
    prelim_buf: Option<Vec<f32>>,
    opts: MatOpts,
    /// Base of the current column in the vein volume (`ORE_IDX`), and block index.
    ore_col: Option<usize>,
    ore_at: Option<usize>,
    /// Veins computed on demand (top material of the caves, `MAT_REUSE`).
    lazy: bool,
    /// Class of each y row of the world from `skip_y0` (`SURF_SKIP`), empty otherwise.
    skip: Vec<u8>,
    skip_y0: i32,
    /// Column constants (`MAT_COLCACHE`): band offset, temperature noise,
    /// frozen part.
    band_off: Option<i32>,
    temp_noise: Option<f32>,
    frozen_cold: Option<bool>,
    /// Free buffers taken back by the next pre-filling.
    spare: Vec<Vec<f32>>,
}

thread_local! {
    /// Evaluation state reused per thread (`MAT_REUSE`).
    static STATE: RefCell<Option<MatState>> = const { RefCell::new(None) };
}

impl MatState {
    fn empty(opts: MatOpts) -> MatState {
        let v = Volume::new(1, 1, 1, 0, 0, 0);
        MatState {
            x: 0,
            z: 0,
            grad_x: 0,
            grad_z: 0,
            sd: 0,
            secondary: None,
            min_surface: None,
            y: 0,
            water: i32::MIN,
            below: 0,
            above: 0,
            biome: None,
            xz_stamp: 0,
            y_stamp: 0,
            noise_val: Vec::new(),
            noise_stamp: Vec::new(),
            biome_static: Vec::new(),
            ore_vol: v,
            ore_bufs: Vec::new(),
            prelim_vol: v,
            prelim_buf: None,
            opts,
            ore_col: None,
            ore_at: None,
            lazy: false,
            skip: Vec::new(),
            skip_y0: 0,
            band_off: None,
            temp_noise: None,
            frozen_cold: None,
            spare: Vec::new(),
        }
    }

    /// Buffer of `n` elements taken from the free buffers.
    fn buf(&mut self, n: usize) -> Vec<f32> {
        let mut b = self.spare.pop().unwrap_or_default();
        b.clear();
        b.resize(n, 0.0);
        b
    }
}

/// State of the thread (`reuse`) or fresh, handed back to the thread after `f`.
fn with_state<R>(reuse: bool, opts: MatOpts, f: impl FnOnce(&mut MatState) -> R) -> R {
    if !reuse {
        return f(&mut MatState::empty(opts));
    }
    let mut st = STATE.with(|s| s.borrow_mut().take()).unwrap_or_else(|| MatState::empty(opts));
    let r = f(&mut st);
    STATE.with(|s| *s.borrow_mut() = Some(st));
    r
}

/// Environment of an evaluation.
pub struct Env<'a, 'b> {
    pub g: &'a Generator,
    pub ctx: &'a mut Ctx,
    pub biomes: &'a mut BiomeView<'b>,
}

impl MaterialSystem {
    /// Prepares the rule `rule` (identifier or object) of the setting, its noises and its functions.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pack: &DataPack,
        g: &mut Generator,
        blocks: &mut BlockTable,
        rule: &Json,
        biome_names: &[String],
        gc: GenContext,
        default_block: BlockId,
    ) -> Result<Self, Error> {
        let biomes: HashMap<String, u16> = biome_names.iter().enumerate().map(|(i, n)| (n.clone(), i as u16)).collect();
        let mut b = Builder {
            pack,
            g,
            blocks,
            biomes: &biomes,
            n_biomes: biome_names.len(),
            gc,
            rules: Vec::new(),
            conds: Vec::new(),
            ores: Vec::new(),
            named_rules: HashMap::new(),
            named_conds: HashMap::new(),
            noise_slots: Vec::new(),
            biome_slots: 0,
        };
        let root = b.rule(rule)?;
        let (mut rules, conds, ores, noise_slots, biome_slots) =
            (b.rules, b.conds, b.ores, b.noise_slots, b.biome_slots);
        let mut prefill = Vec::new();
        prefill_order(&rules, root, &mut prefill);
        let mut n = |id: &str| g.noise(id);
        let surface = n("minecraft:surface")?;
        let secondary = n("minecraft:surface_secondary")?;
        let clay_offset = n("minecraft:clay_bands_offset")?;
        let badlands =
            [n("minecraft:badlands_pillar")?, n("minecraft:badlands_pillar_roof")?, n("minecraft:badlands_surface")?];
        let iceberg =
            [n("minecraft:iceberg_pillar")?, n("minecraft:iceberg_pillar_roof")?, n("minecraft:iceberg_surface")?];
        // Dead veins per y row, then y bands with constant facts and their specialised trees.
        let gates: Vec<Option<YGate>> = ores.iter().map(|o| gate_of(&g.program, o.density)).collect();
        let n_ores = ores.len();
        let (y_min, depth) = (gc.min_y, gc.depth.max(0));
        let mut dead = vec![false; depth as usize * n_ores];
        let mut ctx = Ctx::uncached();
        for k in 0..depth as usize {
            for (i, gate) in gates.iter().enumerate() {
                if let Some(gt) = gate {
                    let v = g.value(gt.input, &mut ctx, 0, y_min + k as i32, 0);
                    dead[k * n_ores + i] = !(v >= gt.min && v < gt.max);
                }
            }
        }
        let mut cuts = vec![y_min, y_min + depth];
        for c in &conds {
            match c {
                Cond::VerticalGradient { t, f, .. } => cuts.extend([t.saturating_add(1), *f]),
                Cond::YAbove { anchor, mult: 0, add: false } => cuts.push(*anchor),
                _ => {}
            }
        }
        let row = |k: usize| &dead[k * n_ores..(k + 1) * n_ores];
        cuts.extend((1..depth as usize).filter(|k| row(*k) != row(k - 1)).map(|k| y_min + k as i32));
        cuts.retain(|y| (y_min..=y_min + depth).contains(y));
        cuts.sort_unstable();
        cuts.dedup();
        let mut y_bands = Vec::new();
        let mut band_of = vec![0u16; depth as usize];
        for w in cuts.windows(2) {
            let (a, e) = (w[0], w[1]);
            let k = (a - y_min) as usize;
            let f = Facts { y: a, dead: row(k), biome_static: None, below_surface: false };
            let r = specialize(&mut rules, &conds, root, &f, &mut HashMap::new()).unwrap_or(NO_RULE);
            band_of[k..(e - y_min) as usize].fill(y_bands.len() as u16);
            y_bands.push((a, e, r));
        }
        let random = g.positional();
        let bands = generate_bands(&mut random.from_hash_of("minecraft:clay_bands"), blocks);
        let mut climates = vec![BiomeClimate::default(); biome_names.len()];
        for (i, name) in biome_names.iter().enumerate() {
            if let Ok(j) = pack.json("biome", name) {
                climates[i] = BiomeClimate {
                    temperature: j.get("temperature").and_then(Json::as_f32).unwrap_or(0.0),
                    frozen: j.get("temperature_modifier").and_then(Json::as_str) == Some("frozen"),
                };
            }
        }
        Ok(MaterialSystem {
            rules,
            conds,
            ores,
            root,
            prefill,
            noise_slots,
            biome_slots,
            surface,
            secondary,
            clay_offset,
            badlands,
            iceberg,
            random,
            ore_random: random.from_hash_of("minecraft:ore").fork_positional(),
            bands,
            default_block,
            sea: gc.sea,
            chunk_surface: g.router.chunk_surface_level,
            snow_block: blocks.intern("minecraft:snow_block"),
            packed_ice: blocks.intern("minecraft:packed_ice"),
            eroded_badlands: biomes.get("minecraft:eroded_badlands").copied(),
            frozen_oceans: [
                biomes.get("minecraft:frozen_ocean").copied(),
                biomes.get("minecraft:deep_frozen_ocean").copied(),
            ],
            climates,
            temps: TemperatureNoises::new(),
            opts: MatOpts::from_env(),
            gates,
            y_min,
            depth,
            dead,
            y_bands,
            band_of,
        })
    }

    /// y bands with constant facts and their specialised root (`None`: surely "nothing"), for
    /// the tests.
    pub fn band_roots(&self) -> Vec<(i32, i32, Option<RuleId>)> {
        self.y_bands.iter().map(|&(a, e, r)| (a, e, (r != NO_RULE).then_some(r))).collect()
    }

    /// y range `[min, max[` of the threshold function of each vein whose threshold is proven, and
    /// constant outside the range (tests).
    pub fn ore_gates(&self) -> Vec<Option<(f32, f32, f32)>> {
        self.gates.iter().map(|g| g.map(|g| (g.min, g.max, g.outside))).collect()
    }

    /// Buffers (density, richness) of the vein pre-filling over `v` through the paths `opts`
    /// (equivalence tests; richness only makes sense where the density is not ≤ 0).
    #[doc(hidden)]
    pub fn prefill_with(&self, env: &mut Env, v: Volume, opts: MatOpts) -> Vec<Option<(Vec<f32>, Vec<f32>)>> {
        let mut st = MatState::empty(opts);
        st.ore_vol = v;
        st.ore_bufs.resize(self.ores.len(), None);
        for &i in &self.prefill {
            self.fill_ore(i, env, &mut st);
        }
        st.ore_bufs
    }

    /// "Cold" (`temperature < 0.15`) by direct computation and by the path kept per column,
    /// for each `y` of a single column (equivalence tests).
    #[doc(hidden)]
    pub fn cold_both(&self, biome: u16, x: i32, z: i32, ys: &[i32]) -> Vec<(bool, bool)> {
        let mut st = MatState::empty(self.opts);
        st.x = x;
        st.z = z;
        ys.iter()
            .map(|&y| {
                st.y = y;
                (self.temperature(biome, x, y, z) < 0.15, self.cold_cached(biome, &mut st))
            })
            .collect()
    }

    /// Density of vein `i` surely constant ≤ 0 at row `y` (outside the table: no).
    #[inline]
    fn ore_dead(&self, i: u32, y: i32) -> bool {
        let k = y.wrapping_sub(self.y_min);
        k >= 0 && k < self.depth && self.dead[k as usize * self.ores.len() + i as usize]
    }

    /// Root to evaluate for row `y` (`SURF_BANDS`).
    #[inline]
    fn root_at(&self, y: i32) -> RuleId {
        let k = y.wrapping_sub(self.y_min);
        if self.opts.surf_bands && k >= 0 && k < self.depth {
            self.y_bands[self.band_of[k as usize] as usize].2
        } else {
            self.root
        }
    }

    #[inline]
    fn noise(&self, g: &Generator, n: u32, x: f64, y: f64, z: f64) -> f32 {
        g.program.noises[n as usize].get(x, y, z)
    }

    /// "Compiled" context for an expected volume: vein pre-filling (game order) and
    /// shortcut of the biome conditions (`possible` = biomes present around, `None` = none).
    pub fn compile(&self, env: &mut Env, expected: Volume, possible: Option<&[bool]>) -> MatState {
        let mut st = MatState::empty(self.opts);
        self.compile_into(env, expected, possible, false, &mut st);
        st
    }

    /// [`MaterialSystem::compile`] into an existing state (buffers taken back); `lazy` = veins
    /// computed at first need instead of pre-filling.
    fn compile_into(&self, env: &mut Env, expected: Volume, possible: Option<&[bool]>, lazy: bool, st: &mut MatState) {
        st.opts = self.opts;
        for (d, r) in st.ore_bufs.drain(..).flatten() {
            st.spare.extend([d, r]);
        }
        st.spare.extend(st.prelim_buf.take());
        st.ore_bufs.resize(self.ores.len(), None);
        st.ore_vol = expected;
        st.lazy = lazy;
        if !lazy {
            for &i in &self.prefill {
                self.fill_ore(i, env, st);
            }
        }
        st.biome_static.clear();
        st.biome_static.resize(self.biome_slots as usize, 0);
        if let Some(p) = possible {
            for c in &self.conds {
                if let Cond::Biome { set, slot } = c {
                    let never = !set.iter().zip(p).any(|(s, q)| *s && *q);
                    let always = p.iter().zip(set).all(|(q, s)| !*q || *s);
                    st.biome_static[*slot as usize] = if never {
                        2
                    } else if always {
                        1
                    } else {
                        0
                    };
                }
            }
        }
        st.x = 0;
        st.z = 0;
        st.grad_x = 0;
        st.grad_z = 0;
        st.sd = 0;
        st.secondary = None;
        st.min_surface = None;
        st.y = 0;
        st.water = i32::MIN;
        st.below = 0;
        st.above = 0;
        st.biome = None;
        st.xz_stamp = 0;
        st.y_stamp = 0;
        st.noise_val.clear();
        st.noise_val.resize(self.noise_slots.len(), 0.0);
        st.noise_stamp.clear();
        st.noise_stamp.resize(self.noise_slots.len(), u64::MAX);
        st.prelim_vol = Volume::new(expected.sx, 1, expected.sz, expected.x0, 0, expected.z0);
        st.ore_col = None;
        st.ore_at = None;
        st.band_off = None;
        st.temp_noise = None;
        st.frozen_cold = None;
        st.skip.clear();
        // Only for a chunk (biomes known): without them, the bands are enough.
        if self.opts.surf_skip && possible.is_some() {
            self.classify_rows(st);
        }
    }

    /// Rows of the vein table to compute for `ores` (envelope of the live rows of the
    /// volume): `None` = none, `Some((0, sy))` = all.
    fn live_rows(&self, ores: impl Iterator<Item = u32>, v: &Volume) -> Option<(i32, i32)> {
        let (mut lo, mut hi) = (v.sy, 0);
        for i in ores {
            if self.gates[i as usize].is_none() {
                return Some((0, v.sy));
            }
            for r in 0..v.sy {
                if !self.ore_dead(i, v.by(r)) {
                    lo = lo.min(r);
                    hi = hi.max(r + 1);
                }
            }
        }
        (lo < hi).then_some((lo, hi))
    }

    /// Density and richness of vein `i` over the vein volume (pre-filling).
    fn fill_ore(&self, i: u32, env: &mut Env, st: &mut MatState) {
        let o = &self.ores[i as usize];
        let v = st.ore_vol;
        let n = v.size();
        let p = &env.g.program;
        if !st.opts.vein_yrange || self.gates[i as usize].is_none() {
            let mut d = st.buf(n);
            p.volume(o.density, env.ctx, &mut d, &v);
            let mut r = st.buf(n);
            p.volume(o.richness, env.ctx, &mut r, &v);
            st.ore_bufs[i as usize] = Some((d, r));
            return;
        }
        let rows = if st.opts.vein_shared {
            self.live_rows(self.prefill.iter().copied(), &v)
        } else {
            self.live_rows(std::iter::once(i), &v)
        };
        // Dead rows: the constant outside the range, exact value of the choice on y (point and volume).
        let mut d = st.buf(n);
        d.fill(self.gates[i as usize].map_or(0.0, |g| g.outside));
        p.volume_rows(o.density, env.ctx, &mut d, &v, rows);
        // Richness is only read where the density is not ≤ 0 (NaN included).
        let rich_rows = if st.opts.vein_lazyrich {
            crate::sampler::rows_hull(&v, &d, |x| x > 0.0 || x.is_nan())
        } else {
            rows
        };
        let mut r = st.buf(if rich_rows.is_some() { n } else { 0 });
        p.volume_rows(o.richness, env.ctx, &mut r, &v, rich_rows);
        st.ore_bufs[i as usize] = Some((d, r));
    }

    /// Class of each y row of the world for the root (`SURF_SKIP`), band by band.
    fn classify_rows(&self, st: &mut MatState) {
        st.skip_y0 = self.y_min;
        st.skip.resize(self.depth as usize, LIVE);
        let n = self.ores.len();
        for &(a, e, _) in &self.y_bands {
            let k = (a - self.y_min) as usize;
            let mut f = Facts {
                y: a,
                dead: &self.dead[k * n..(k + 1) * n],
                biome_static: Some(&st.biome_static),
                below_surface: true,
            };
            let class = if surely_none(&self.rules, &self.conds, self.root, &f) {
                f.below_surface = false;
                if surely_none(&self.rules, &self.conds, self.root, &f) { DEAD } else { DEAD_BELOW }
            } else {
                LIVE
            };
            st.skip[k..(e - self.y_min) as usize].fill(class);
        }
    }

    /// Does the root surely return "nothing" at the current block (`SURF_SKIP`)?
    #[inline]
    fn skip_row(st: &MatState) -> bool {
        let k = st.y.wrapping_sub(st.skip_y0);
        match st.skip.get(k as usize) {
            Some(&DEAD) => k >= 0,
            Some(&DEAD_BELOW) => k >= 0 && st.min_surface.is_some_and(|m| st.y < m),
            _ => false,
        }
    }

    /// Surface depth of a column.
    pub fn surface_depth(&self, g: &Generator, x: i32, z: i32) -> i32 {
        let n = f64::from(self.noise(g, self.surface, f64::from(x), 0.0, f64::from(z)));
        (n * 2.75 + 3.0 + self.random.at(x, 0, z).next_double() * 0.25) as i32
    }

    pub fn update_xz(&self, g: &Generator, st: &mut MatState, x: i32, z: i32, grad_x: i32, grad_z: i32) {
        st.xz_stamp += 1;
        st.y_stamp += 1;
        st.x = x;
        st.z = z;
        st.grad_x = grad_x;
        st.grad_z = grad_z;
        st.sd = self.surface_depth(g, x, z);
        st.secondary = None;
        st.min_surface = None;
        st.band_off = None;
        st.temp_noise = None;
        st.frozen_cold = None;
        if st.opts.ore_idx {
            let v = &st.ore_vol;
            let (rx, rz) = (x.wrapping_sub(v.x0), z.wrapping_sub(v.z0));
            st.ore_col = (v.stx == 1 && v.sty == 1 && v.stz == 1 && rx >= 0 && rz >= 0 && rx < v.sx && rz < v.sz)
                .then(|| ((rx + rz * v.sx) * v.sy) as usize);
        }
    }

    #[inline]
    pub fn update_y(st: &mut MatState, above: i32, below: i32, water: i32, y: i32) {
        st.y_stamp += 1;
        st.biome = None;
        st.y = y;
        st.water = water;
        st.below = below;
        st.above = above;
        if st.opts.ore_idx {
            let ry = y.wrapping_sub(st.ore_vol.y0);
            st.ore_at = st.ore_col.filter(|_| ry >= 0 && ry < st.ore_vol.sy).map(|b| b + ry as usize);
        }
    }

    fn min_surface(&self, env: &mut Env, st: &mut MatState) -> i32 {
        if let Some(v) = st.min_surface {
            return v;
        }
        let p = match st.prelim_vol.index_of_block(st.x, 0, st.z) {
            Some(i) => {
                if st.prelim_buf.is_none() {
                    let mut b = st.buf(st.prelim_vol.size());
                    env.g.program.volume(self.chunk_surface, env.ctx, &mut b, &st.prelim_vol);
                    st.prelim_buf = Some(b);
                }
                st.prelim_buf.as_ref().map_or(0.0, |b| b[i])
            }
            None => env.g.value(self.chunk_surface, env.ctx, st.x, 0, st.z),
        };
        let v = floor_f(p) + st.sd - 8;
        st.min_surface = Some(v);
        v
    }

    fn biome(&self, env: &mut Env, st: &mut MatState) -> u16 {
        if let Some(b) = st.biome {
            return b;
        }
        let b = env.biomes.get(env.g, st.x, st.y, st.z);
        st.biome = Some(b);
        b
    }

    /// Temperature of a biome at the block ("frozen" modifier, altitude correction).
    pub fn temperature(&self, biome: u16, x: i32, y: i32, z: i32) -> f32 {
        let c = self.climates.get(biome as usize).copied().unwrap_or_default();
        let mut t = c.temperature;
        if c.frozen {
            let large = f64::from(self.temps.frozen_stack(f64::from(x) * 0.05, f64::from(z) * 0.05) * 7.0);
            let edge = f64::from(self.temps.info.get2(f64::from(x) * 0.2, f64::from(z) * 0.2));
            if large + edge < 0.3 {
                let small = f64::from(self.temps.info.get2(f64::from(x) * 0.09, f64::from(z) * 0.09));
                if small < 0.8 {
                    t = 0.2;
                }
            }
        }
        let snow_level = self.sea + 17;
        if y > snow_level {
            let v = self.temps.temperature.get2(f64::from(x as f32 / 8.0), f64::from(z as f32 / 8.0)) * 8.0;
            t - (v + y as f32 - snow_level as f32) * 0.05 / 40.0
        } else {
            t
        }
    }

    fn test(&self, c: CondId, env: &mut Env, st: &mut MatState) -> bool {
        match &self.conds[c as usize] {
            Cond::Biome { set, slot } => match st.biome_static[*slot as usize] {
                1 => true,
                2 => false,
                _ => {
                    let b = self.biome(env, st);
                    set.get(b as usize).copied().unwrap_or(false)
                }
            },
            Cond::Noise { slot, min, max } => {
                let s = *slot as usize;
                let (n, is3d) = self.noise_slots[s];
                let stamp = if is3d { st.y_stamp } else { st.xz_stamp };
                if st.noise_stamp[s] != stamp {
                    let y = if is3d { f64::from(st.y) } else { 0.0 };
                    st.noise_val[s] = f64::from(self.noise(env.g, n, f64::from(st.x), y, f64::from(st.z)));
                    st.noise_stamp[s] = stamp;
                }
                let v = st.noise_val[s];
                v >= *min && v <= *max
            }
            Cond::VerticalGradient { t, f, random } => {
                let y = st.y;
                if y <= *t {
                    return true;
                }
                if y >= *f {
                    return false;
                }
                let p = crate::aquifer::map(f64::from(y), f64::from(*t), f64::from(*f), 1.0, 0.0);
                f64::from(random.at(st.x, y, st.z).next_float()) < p
            }
            Cond::YAbove { anchor, mult, add } => st.y + if *add { st.above } else { 0 } >= anchor + st.sd * mult,
            Cond::Water { offset, mult, add } => {
                st.water == i32::MIN || st.y + if *add { st.above } else { 0 } >= st.water + offset + st.sd * mult
            }
            Cond::Temperature => {
                let b = self.biome(env, st);
                if self.opts.colcache {
                    self.cold_cached(b, st)
                } else {
                    self.temperature(b, st.x, st.y, st.z) < 0.15
                }
            }
            Cond::Steep => st.grad_x <= -4 || st.grad_z >= 4,
            Cond::Not(i) => !self.test(*i, env, st),
            Cond::Hole => st.sd <= 0,
            Cond::AbovePreliminarySurface => st.y >= self.min_surface(env, st),
            Cond::StoneDepth { offset, add_sd, range, ceiling } => {
                let depth = if *ceiling { st.below } else { st.above };
                let sd = if *add_sd { st.sd } else { 0 };
                let sec = if *range == 0 {
                    0
                } else {
                    let s = match st.secondary {
                        Some(v) => v,
                        None => {
                            let v = f64::from(self.noise(env.g, self.secondary, f64::from(st.x), 0.0, f64::from(st.z)));
                            st.secondary = Some(v);
                            v
                        }
                    };
                    crate::aquifer::map(s, -1.0, 1.0, 0.0, f64::from(*range)) as i32
                };
                depth <= 1 + offset + sd + sec
            }
        }
    }

    /// `temperature(..) < 0.15` with the column constants kept (frozen part, altitude
    /// noise) and the noise skipped when the bound proves the correction does not cool enough.
    fn cold_cached(&self, biome: u16, st: &mut MatState) -> bool {
        let c = self.climates.get(biome as usize).copied().unwrap_or_default();
        let mut t = c.temperature;
        if c.frozen {
            let (x, z) = (st.x, st.z);
            let cold = *st.frozen_cold.get_or_insert_with(|| {
                let large = f64::from(self.temps.frozen_stack(f64::from(x) * 0.05, f64::from(z) * 0.05) * 7.0);
                let edge = f64::from(self.temps.info.get2(f64::from(x) * 0.2, f64::from(z) * 0.2));
                large + edge < 0.3 && f64::from(self.temps.info.get2(f64::from(x) * 0.09, f64::from(z) * 0.09)) < 0.8
            });
            if cold {
                t = 0.2;
            }
        }
        let snow_level = self.sea + 17;
        if st.y <= snow_level {
            return t < 0.15;
        }
        // Same sequence of f32 operations as the exact computation, monotonic in v: v ≤ bound ⇒ result ≥.
        let bound = 8.0 * TEMPERATURE_NOISE_BOUND;
        if t - (bound + st.y as f32 - snow_level as f32) * 0.05 / 40.0 >= 0.15 {
            return false;
        }
        let (x, z) = (st.x, st.z);
        let v = *st
            .temp_noise
            .get_or_insert_with(|| self.temps.temperature.get2(f64::from(x as f32 / 8.0), f64::from(z as f32 / 8.0)) * 8.0);
        t - (v + st.y as f32 - snow_level as f32) * 0.05 / 40.0 < 0.15
    }

    fn band(&self, g: &Generator, x: i32, y: i32, z: i32) -> BlockId {
        let o = round_half_up(self.noise(g, self.clay_offset, f64::from(x), 0.0, f64::from(z)) * 4.0);
        self.bands[((y + o + 192) % 192) as usize]
    }

    /// [`MaterialSystem::band`] with the column offset kept (`MAT_COLCACHE`).
    fn band_cached(&self, g: &Generator, st: &mut MatState) -> BlockId {
        let (x, z) = (st.x, st.z);
        let o = *st
            .band_off
            .get_or_insert_with(|| round_half_up(self.noise(g, self.clay_offset, f64::from(x), 0.0, f64::from(z)) * 4.0));
        self.bands[((st.y + o + 192) % 192) as usize]
    }

    fn ore(&self, i: u32, env: &mut Env, st: &mut MatState) -> Option<BlockId> {
        if self.opts.ore_early && self.ore_dead(i, st.y) {
            return None;
        }
        let o = &self.ores[i as usize];
        let idx = if self.opts.ore_idx { st.ore_at } else { st.ore_vol.index_of_block(st.x, st.y, st.z) };
        if st.lazy && idx.is_some() && st.ore_bufs[i as usize].is_none() {
            self.fill_ore(i, env, st);
        }
        let density = match (idx, &st.ore_bufs[i as usize]) {
            (Some(j), Some(b)) => b.0[j],
            _ => env.g.value(o.density, env.ctx, st.x, st.y, st.z),
        };
        if density <= 0.0 {
            return None;
        }
        let mut r = self.ore_random.at(st.x, st.y, st.z);
        if r.next_float() > density {
            return None;
        }
        let richness = match (idx, &st.ore_bufs[i as usize]) {
            (Some(j), Some(b)) => b.1[j],
            _ => env.g.value(o.richness, env.ctx, st.x, st.y, st.z),
        };
        if r.next_float() < richness && env.g.value(o.gap, env.ctx, st.x, st.y, st.z) < 0.0 {
            Some(if r.next_float() < o.raw_chance { o.raw } else { o.ore })
        } else {
            Some(o.filler)
        }
    }

    /// Rule `r` at the current block of the state.
    pub fn apply(&self, r: RuleId, env: &mut Env, st: &mut MatState) -> Option<BlockId> {
        match &self.rules[r as usize] {
            Rule::Block(b) => Some(*b),
            Rule::Sequence(v) => {
                for c in v {
                    if let Some(b) = self.apply(*c, env, st) {
                        return Some(b);
                    }
                }
                None
            }
            Rule::Condition(c, t) => {
                if self.test(*c, env, st) {
                    self.apply(*t, env, st)
                } else {
                    None
                }
            }
            Rule::Bandlands => Some(if self.opts.colcache {
                self.band_cached(env.g, st)
            } else {
                self.band(env.g, st.x, st.y, st.z)
            }),
            Rule::Ore(i) => self.ore(*i, env, st),
        }
    }

    /// Root rule at the current block, through the chosen paths (dead row skip, bands).
    #[inline]
    pub fn apply_root(&self, env: &mut Env, st: &mut MatState) -> Option<BlockId> {
        if self.opts.surf_skip && Self::skip_row(st) {
            return None;
        }
        match self.root_at(st.y) {
            NO_RULE => None,
            r => self.apply(r, env, st),
        }
    }

    /// Surface walk of a chunk (§ 4.2). `full` = volume of the noise filling.
    pub fn build_surface(
        &self,
        chunk: &mut ProtoTerrain,
        blocks: &BlockTable,
        env: &mut Env,
        full: Volume,
        possible: Option<&[bool]>,
    ) {
        let min_y = chunk.min_y;
        let top_section = chunk.highest_filled_section().map_or(-1, |s| s as i32);
        let max_block_y = min_y + top_section * 16 + 15;
        let narrowed = Volume::new(full.sx, (max_block_y - full.y0 + 1).max(1), full.sz, full.x0, full.y0, full.z0);
        with_state(self.opts.reuse, self.opts, |st| {
            self.compile_into(env, narrowed, possible, false, st);
            self.surface_columns(chunk, blocks, env, st);
        });
    }

    fn surface_columns(&self, chunk: &mut ProtoTerrain, blocks: &BlockTable, env: &mut Env, st: &mut MatState) {
        let (min_y, max_y) = (chunk.min_y, chunk.max_y());
        let (bx0, bz0) = (chunk.min_x(), chunk.min_z());
        for x in 0..16 {
            for z in 0..16 {
                let (bx, bz) = (bx0 + x, bz0 + z);
                let start = chunk.surface_height(x, z) + 1;
                let surface_biome = env.biomes.get(env.g, bx, start, bz);
                if Some(surface_biome) == self.eroded_badlands {
                    self.eroded_badlands_extension(chunk, blocks, env.g, x, z, start);
                }
                let height = chunk.surface_height(x, z) + 1;
                let (gx, gz) = gradients(chunk, x, z);
                self.update_xz(env.g, st, bx, bz, gx, gz);
                let mut above = 0;
                let mut water = i32::MIN;
                let mut ceiling = i32::MAX;
                let end = min_y;
                let mut y = height;
                while y >= end {
                    let old = chunk.get(x, y, z);
                    if blocks.is_air(old) {
                        above = 0;
                        water = i32::MIN;
                    } else if blocks.has_fluid(old) {
                        if water == i32::MIN {
                            water = y + 1;
                        }
                    } else {
                        if ceiling >= y {
                            ceiling = crate::aquifer::WAY_BELOW_MIN_Y;
                            let mut ly = y - 1;
                            while ly >= end - 1 {
                                let s = chunk.get(x, ly, z);
                                if blocks.is_air(s) || blocks.has_fluid(s) {
                                    ceiling = ly + 1;
                                    break;
                                }
                                ly -= 1;
                            }
                        }
                        above += 1;
                        let below = y - ceiling + 1;
                        Self::update_y(st, above, below, water, y);
                        if y >= min_y
                            && y <= max_y
                            && let Some(s) = self.apply_root(env, st)
                        {
                            set_column(chunk, blocks, x, y, z, s);
                        }
                    }
                    y -= 1;
                }
                if self.frozen_oceans.contains(&Some(surface_biome)) {
                    let ms = self.min_surface(env, st);
                    self.frozen_ocean_extension(chunk, blocks, env.g, ms, surface_biome, x, z, start);
                }
            }
        }
    }

    fn eroded_badlands_extension(
        &self,
        chunk: &mut ProtoTerrain,
        blocks: &BlockTable,
        g: &Generator,
        x: i32,
        z: i32,
        height: i32,
    ) {
        let (bx, bz) = (f64::from(chunk.min_x() + x), f64::from(chunk.min_z() + z));
        let [pillar, roof, surface] = self.badlands;
        let a = (f64::from(self.noise(g, surface, bx, 0.0, bz)) * 8.25).abs();
        let b = f64::from(self.noise(g, pillar, bx * 0.2, 0.0, bz * 0.2) * 15.0);
        let buffer = a.min(b);
        if buffer <= 0.0 {
            return;
        }
        let floor = (f64::from(self.noise(g, roof, bx * 0.75, 0.0, bz * 0.75)) * 1.5).abs();
        let top = 64.0 + (buffer * buffer * 2.5).min((floor * 50.0).ceil() + 24.0);
        let start = top.floor() as i32;
        if height > start {
            return;
        }
        let mut y = start;
        while y >= chunk.min_y {
            let old = chunk.get(x, y, z);
            if blocks.same_block(old, self.default_block) {
                break;
            }
            if blocks.is_water(old) {
                return;
            }
            y -= 1;
        }
        let mut y = start;
        while y >= chunk.min_y && blocks.is_air(chunk.get(x, y, z)) {
            set_column(chunk, blocks, x, y, z, self.default_block);
            y -= 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frozen_ocean_extension(
        &self,
        chunk: &mut ProtoTerrain,
        blocks: &BlockTable,
        g: &Generator,
        min_surface: i32,
        biome: u16,
        x: i32,
        z: i32,
        height: i32,
    ) {
        let (ix, iz) = (chunk.min_x() + x, chunk.min_z() + z);
        let (bx, bz) = (f64::from(ix), f64::from(iz));
        let [pillar, roof, surface] = self.iceberg;
        let a = (f64::from(self.noise(g, surface, bx, 0.0, bz)) * 8.25).abs();
        let b = f64::from(self.noise(g, pillar, bx * 1.28, 0.0, bz * 1.28) * 15.0);
        let iceberg = a.min(b);
        if iceberg <= 1.8 {
            return;
        }
        let r = (f64::from(self.noise(g, roof, bx * 1.17, 0.0, bz * 1.17)) * 1.5).abs();
        let mut top = (iceberg * iceberg * 1.2).min((r * 40.0).ceil() + 14.0);
        if self.temperature(biome, ix, self.sea, iz) > 0.1 {
            top -= 2.0;
        }
        if top <= 2.0 {
            return;
        }
        let bottom = f64::from(self.sea) - top - 7.0;
        top += f64::from(self.sea);
        let mut rnd = self.random.at(ix, 0, iz);
        let max_snow = 2 + rnd.next_int_bounded(4);
        let min_snow_y = self.sea + 18 + rnd.next_int_bounded(10);
        let mut snow = 0;
        let mut y = height.max(top as i32 + 1);
        while y >= min_surface {
            let s = chunk.get(x, y, z);
            let place = (blocks.is_air(s) && y < top as i32 && rnd.next_double() > 0.01)
                || (blocks.is_water(s) && y > bottom as i32 && y < self.sea && rnd.next_double() > 0.15);
            if place {
                if snow <= max_snow && y > min_snow_y {
                    set_column(chunk, blocks, x, y, z, self.snow_block);
                    snow += 1;
                } else {
                    set_column(chunk, blocks, x, y, z, self.packed_ice);
                }
            }
            y -= 1;
        }
    }

    /// Top material replayed by the caves (local block x, z; world y): rule recompiled
    /// for a one-block volume, like the game.
    pub fn top_material(
        &self,
        chunk: &ProtoTerrain,
        env: &mut Env,
        x: i32,
        y: i32,
        z: i32,
        under_fluid: bool,
    ) -> Option<BlockId> {
        let (bx, bz) = (chunk.min_x() + x, chunk.min_z() + z);
        let reuse = self.opts.reuse;
        with_state(reuse, self.opts, |st| {
            self.compile_into(env, Volume::new(1, 1, 1, bx, y, bz), None, reuse, st);
            let (gx, gz) = gradients(chunk, x, z);
            self.update_xz(env.g, st, bx, bz, gx, gz);
            Self::update_y(st, 1, 1, if under_fluid { y + 1 } else { i32::MIN }, y);
            self.apply_root(env, st)
        })
    }
}

/// Slopes of the column read on `WORLD_SURFACE_WG` (chunk bounds repeated).
pub fn gradients(chunk: &ProtoTerrain, x: i32, z: i32) -> (i32, i32) {
    let h = |x: i32, z: i32| chunk.surface_height(x, z);
    (h((x + 1).min(15), z) - h((x - 1).max(0), z), h(x, (z + 1).min(15)) - h(x, (z - 1).max(0)))
}

/// Placement of a surface column: within the world height, with mark if fluid.
fn set_column(chunk: &mut ProtoTerrain, blocks: &BlockTable, x: i32, y: i32, z: i32, s: BlockId) {
    if y >= chunk.min_y && y <= chunk.max_y() {
        chunk.set(blocks, x, y, z, s);
        if blocks.has_fluid(s) {
            chunk.mark_post_processing(x, y, z);
        }
    }
}

/// Biomes present in a list of chunks (shortcut of the biome conditions).
pub fn possible_biomes(n: usize, chunks: &[&ChunkBiomes]) -> Vec<bool> {
    let mut p = vec![false; n];
    for c in chunks {
        for b in &c.data {
            if let Some(v) = p.get_mut(*b as usize) {
                *v = true;
            }
        }
    }
    p
}
