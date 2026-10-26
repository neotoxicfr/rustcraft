//! Placement of the structures during decoration (`generation-decoration.md` § 9.2, § 10.3–10.5):
//! template processors, placement of a template (waterlogging, block entities), jigsaw pieces,
//! buried treasures; the longer coded pieces are in `structure_*.rs`.

use hashbrown::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use rc_proto::nbt::Nbt;

use crate::decor::Ctx;
use crate::decor_blocks::{Blocks, State};
use crate::decor_data::DecorData;
use crate::decor_pred::{IntP, Rule};
use crate::decor_region::{BeData, Hm, Region};
use crate::decor_rng::DecoRng;
use crate::jigsaw::{Element, JigsawPiece, Projection};
use crate::json::Json;
use crate::structure::{Piece, Start, StructureGen};
use crate::structure_coded::CodedPiece;
use crate::structure_procs::ProcTables;
use crate::template::{BBox, Mirror, Rot, Template, transform};
use crate::terrain::TerrainGenerator;

// ---------------------------------------------------------------- Processors

#[derive(Debug, Clone)]
pub enum PosPred {
    Always,
    /// (min chance, max chance, min distance, max distance, axis 0..2).
    Linear(f32, f32, i32, i32, usize),
}

#[derive(Debug, Clone)]
pub enum BeMod {
    Pass,
    Clear,
    Static(Nbt),
    Loot(String),
}

#[derive(Debug, Clone)]
pub struct PRule {
    input: Rule,
    loc: Rule,
    pos: PosPred,
    output: State,
    modifier: BeMod,
}

impl PRule {
    /// "Input → output" rule without location or position condition.
    pub fn simple(input: Rule, output: State, modifier: BeMod) -> PRule {
        PRule { input, loc: Rule::Always, pos: PosPred::Always, output, modifier }
    }
}

#[derive(Debug, Clone)]
pub enum Proc {
    Nop,
    Ignore(Vec<u16>),
    Rot { integrity: f32, rottable: Option<u16> },
    Rules(Vec<PRule>),
    Gravity { hm: Hm, offset: i32 },
    JigsawReplace,
    Protected(Option<u16>),
    Capped { delegate: Box<Proc>, limit: IntP },
    Age { mossiness: f32 },
    Blackstone,
    LavaSubmerged,
}

pub(crate) fn json_to_nbt(j: &Json) -> Nbt {
    match j {
        Json::Null => Nbt::Compound(Vec::new()),
        Json::Bool(b) => Nbt::Byte(i8::from(*b)),
        Json::Num(s) => s.parse::<i32>().map(Nbt::Int).unwrap_or_else(|_| Nbt::Double(s.parse().unwrap_or(0.0))),
        Json::Str(s) => Nbt::String(s.clone()),
        Json::Arr(a) => Nbt::List(a.iter().map(json_to_nbt).collect()),
        Json::Obj(m) => Nbt::Compound(m.iter().map(|(k, v)| (k.clone(), json_to_nbt(v))).collect()),
    }
}

fn parse_proc(j: &Json, b: &Blocks) -> Option<Proc> {
    let t = j.get("processor_type")?.as_str()?;
    Some(match t.strip_prefix("minecraft:").unwrap_or(t) {
        "nop" => Proc::Nop,
        "block_ignore" => Proc::Ignore(
            j.get("blocks")?.as_array()?.iter().filter_map(|s| b.parse_state(s).ok().map(|st| b.block(st))).collect(),
        ),
        "block_rot" => Proc::Rot {
            integrity: j.get("integrity").and_then(Json::as_f32).unwrap_or(1.0),
            rottable: j
                .get("rottable_blocks")
                .and_then(Json::as_str)
                .and_then(|t| b.tag(t.strip_prefix('#').unwrap_or(t))),
        },
        "rule" => Proc::Rules(
            j.get("rules")?
                .as_array()?
                .iter()
                .filter_map(|r| {
                    let pos = match r.get("position_predicate") {
                        Some(p)
                            if p.get("predicate_type").and_then(Json::as_str)
                                == Some("minecraft:axis_aligned_linear_pos") =>
                        {
                            let axis = match p.get("axis").and_then(Json::as_str).unwrap_or("y") {
                                "x" => 0,
                                "z" => 2,
                                _ => 1,
                            };
                            PosPred::Linear(
                                p.get("min_chance").and_then(Json::as_f32).unwrap_or(0.0),
                                p.get("max_chance").and_then(Json::as_f32).unwrap_or(0.0),
                                p.get("min_dist").and_then(Json::as_i32).unwrap_or(0),
                                p.get("max_dist").and_then(Json::as_i32).unwrap_or(0),
                                axis,
                            )
                        }
                        _ => PosPred::Always,
                    };
                    let modifier = match r.get("block_entity_modifier") {
                        Some(m) => match m.get("type").and_then(Json::as_str).unwrap_or("") {
                            "minecraft:clear" => BeMod::Clear,
                            "minecraft:append_static" => {
                                BeMod::Static(json_to_nbt(m.get("data").unwrap_or(&Json::Null)))
                            }
                            "minecraft:append_loot" => {
                                BeMod::Loot(m.get("loot_table").and_then(Json::as_str).unwrap_or("").to_owned())
                            }
                            _ => BeMod::Pass,
                        },
                        None => BeMod::Pass,
                    };
                    Some(PRule {
                        input: Rule::parse(r.get("input_predicate")?, b).ok()?,
                        loc: Rule::parse(r.get("location_predicate")?, b).ok()?,
                        pos,
                        output: b.parse_state(r.get("output_state")?).ok()?,
                        modifier,
                    })
                })
                .collect(),
        ),
        "gravity" => Proc::Gravity {
            hm: j.get("heightmap").and_then(Json::as_str).and_then(Hm::from_name).unwrap_or(Hm::WorldSurfaceWg),
            offset: j.get("offset").and_then(Json::as_i32).unwrap_or(0),
        },
        "jigsaw_replacement" => Proc::JigsawReplace,
        "protected_blocks" => {
            Proc::Protected(j.get("value").and_then(Json::as_str).and_then(|t| b.tag(t.strip_prefix('#').unwrap_or(t))))
        }
        "capped" => Proc::Capped {
            delegate: Box::new(parse_proc(j.get("delegate")?, b)?),
            limit: IntP::parse(j.get("limit")?).ok()?,
        },
        "block_age" => Proc::Age { mossiness: j.get("mossiness").and_then(Json::as_f32).unwrap_or(0.0) },
        "blackstone_replace" => Proc::Blackstone,
        "lava_submerged_block" => Proc::LavaSubmerged,
        _ => Proc::Nop,
    })
}

/// Processor lists (`worldgen/processor_list`), analysed on demand.
pub struct ProcLists {
    /// Frozen table: text of the list and list analysed at first need.
    raw: HashMap<String, (Json, ProcCell)>,
    /// Old path (`RUSTCRAFT_OPT_POOLS_TABLE=0`).
    cache: Mutex<HashMap<String, Arc<Vec<Proc>>>>,
}

type ProcCell = OnceLock<Arc<Vec<Proc>>>;

/// Processor chain of a placement: prefix specific to the placement, shared common list, suffix.
#[derive(Debug, Clone, Default)]
pub struct ProcChain {
    pub pre: Vec<Proc>,
    pub shared: Arc<Vec<Proc>>,
    pub post: Vec<Proc>,
}

impl From<Vec<Proc>> for ProcChain {
    fn from(pre: Vec<Proc>) -> ProcChain {
        ProcChain { pre, ..Default::default() }
    }
}

impl ProcChain {
    /// Shared common list (`RUSTCRAFT_OPT_PROCS_SHARED`), or copied (old path).
    pub fn of(list: Arc<Vec<Proc>>) -> ProcChain {
        if crate::opt::procs_shared() { ProcChain { shared: list, ..Default::default() } } else { (*list).clone().into() }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Proc> {
        self.pre.iter().chain(self.shared.iter()).chain(self.post.iter())
    }
}

impl std::fmt::Debug for ProcLists {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProcLists({})", self.raw.len())
    }
}

impl ProcLists {
    pub fn new(d: &DecorData) -> ProcLists {
        let raw = d
            .ids("processor_list")
            .into_iter()
            .filter_map(|id| Some((id.clone(), (d.json("processor_list", &id).ok()?, OnceLock::new()))))
            .collect();
        ProcLists { raw, cache: Mutex::new(HashMap::new()) }
    }

    /// List designated by an identifier, a `{processors}` object or a bare list.
    pub fn get(&self, j: &Json, b: &Blocks) -> Arc<Vec<Proc>> {
        let parse = |j: &Json| -> Vec<Proc> {
            let list = match j {
                Json::Arr(a) => a.as_slice(),
                o => o.get("processors").and_then(Json::as_array).unwrap_or(&[]),
            };
            list.iter().filter_map(|p| parse_proc(p, b)).collect()
        };
        match j {
            Json::Str(id) if crate::opt::pools_table() => {
                match crate::data::with_full_id(id, |k| self.raw.get(k)) {
                    Some((j, cell)) => cell.get_or_init(|| Arc::new(parse(j))).clone(),
                    None => Arc::new(Vec::new()),
                }
            }
            Json::Str(id) => {
                let id = crate::data::full_id(id);
                if let Some(v) = self.cache.lock().ok().and_then(|c| c.get(&id).cloned()) {
                    return v;
                }
                let v = Arc::new(self.raw.get(&id).map(|(j, _)| parse(j)).unwrap_or_default());
                if let Ok(mut c) = self.cache.lock() {
                    c.insert(id, v.clone());
                }
                v
            }
            Json::Null => Arc::new(Vec::new()),
            o => Arc::new(parse(o)),
        }
    }
}

/// Block being processed.
#[derive(Debug, Clone, PartialEq)]
struct BInfo {
    pos: [i32; 3],
    state: State,
    nbt: Option<Nbt>,
}

fn legacy_at(seed_pos: [i32; 3]) -> DecoRng {
    DecoRng::legacy(crate::rng::position_seed(seed_pos[0], seed_pos[1], seed_pos[2]))
}

/// Placement settings of a template.
#[derive(Clone)]
pub struct Placing {
    pub mirror: Mirror,
    pub rot: Rot,
    pub pivot: [i32; 3],
    pub bbox: Option<BBox>,
    pub procs: ProcChain,
    pub waterlog: bool,
    /// Seed of the settings' randomness (otherwise position seeds).
    pub seed: Option<i64>,
    /// Settings randomness = the placement randomness itself ("template" features, fossils).
    pub shared: bool,
    /// Known shape (jigsaw pieces): no shape updates after placement.
    pub known_shape: bool,
    /// Template entities placed (`Some`), with spawn preparation (`Some(true)`).
    pub entities: Option<Option<std::sync::Arc<[&'static str]>>>,
}

impl Placing {
    fn random(&self, p: [i32; 3]) -> DecoRng {
        match self.seed {
            Some(s) => DecoRng::legacy(s),
            None => legacy_at(p),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn process_one(
    shared: &mut Option<&mut DecoRng>,
    p: &Proc,
    r: &Region,
    b: &Blocks,
    reference: [i32; 3],
    orig: [i32; 3],
    cur: BInfo,
    s: &Placing,
    pt: Option<&ProcTables>,
) -> Option<BInfo> {
    match p {
        Proc::Nop => Some(cur),
        Proc::Ignore(list) => (!list.contains(&b.block(cur.state))).then_some(cur),
        Proc::Rot { integrity, rottable } => {
            let concerned = rottable.is_none_or(|t| b.in_tag(cur.state, t));
            if concerned {
                let mut local;
                let rng: &mut DecoRng = match shared {
                    Some(r) => r,
                    None => {
                        local = s.random(cur.pos);
                        &mut local
                    }
                };
                if rng.next_float() > *integrity {
                    return None;
                }
            }
            Some(cur)
        }
        Proc::Rules(rules) => {
            let mut rng = legacy_at(cur.pos);
            let loc = r.get_p(cur.pos);
            for rule in rules {
                if rule.input.test(b, cur.state, cur.pos[1], &mut rng)
                    && rule.loc.test(b, loc, cur.pos[1], &mut rng)
                    && match rule.pos {
                        PosPred::Always => true,
                        PosPred::Linear(cmin, cmax, dmin, dmax, axis) => {
                            let dist = (cur.pos[axis] - reference[axis]).abs();
                            let rnd = rng.next_float();
                            let t = (dist as f32 - dmin as f32) / (dmax as f32 - dmin as f32);
                            let chance = if t < 0.0 {
                                cmin
                            } else if t > 1.0 {
                                cmax
                            } else {
                                cmin + t * (cmax - cmin)
                            };
                            rnd <= chance
                        }
                    }
                {
                    let nbt = match &rule.modifier {
                        BeMod::Pass => cur.nbt.clone(),
                        BeMod::Clear => Some(Nbt::Compound(Vec::new())),
                        BeMod::Static(d) => {
                            let mut n = cur.nbt.clone().unwrap_or(Nbt::Compound(Vec::new()));
                            if let (Nbt::Compound(f), Nbt::Compound(add)) = (&mut n, d) {
                                for (k, v) in add {
                                    f.retain(|(kk, _)| kk != k);
                                    f.push((k.clone(), v.clone()));
                                }
                            }
                            Some(n)
                        }
                        BeMod::Loot(t) => {
                            let mut n = cur.nbt.clone().unwrap_or(Nbt::Compound(Vec::new()));
                            if let Nbt::Compound(f) = &mut n {
                                f.retain(|(k, _)| k != "LootTable" && k != "LootTableSeed");
                                f.push(("LootTable".into(), Nbt::String(t.clone())));
                                f.push(("LootTableSeed".into(), Nbt::Long(rng.next_long())));
                            }
                            Some(n)
                        }
                    };
                    return Some(BInfo { pos: cur.pos, state: rule.output, nbt });
                }
            }
            Some(cur)
        }
        Proc::Gravity { hm, offset } => {
            let y = r.height_at(*hm, cur.pos[0], cur.pos[2]) + offset;
            Some(BInfo { pos: [cur.pos[0], y + orig[1], cur.pos[2]], ..cur })
        }
        Proc::JigsawReplace => {
            let jigsaw = match pt {
                Some(t) => t.jigsaw,
                None => b.block_id("minecraft:jigsaw"),
            };
            if Some(b.block(cur.state)) != jigsaw {
                return Some(cur);
            }
            let Some(n) = &cur.nbt else { return Some(cur) };
            let fs = n.get("final_state").and_then(Nbt::as_str).unwrap_or("minecraft:air");
            let st = b.state_from_str(fs).or_else(|| crate::data::with_full_id(fs, |f| b.state_from_str(f)))?;
            let void = match pt {
                Some(t) => t.structure_void,
                None => b.block_id("minecraft:structure_void"),
            };
            if Some(b.block(st)) == void {
                return None;
            }
            Some(BInfo { pos: cur.pos, state: st, nbt: None })
        }
        Proc::Protected(tag) => {
            let w = r.get_p(cur.pos);
            if tag.is_some_and(|t| b.in_tag(w, t)) { None } else { Some(cur) }
        }
        Proc::Capped { .. } => Some(cur),
        Proc::Age { mossiness } => {
            let mut local;
            let rng: &mut DecoRng = match shared {
                Some(r) => r,
                None => {
                    local = s.random(cur.pos);
                    &mut local
                }
            };
            match pt {
                Some(t) => crate::structure_procs::block_age_fast(b, t, cur.state, *mossiness, rng),
                None => crate::structure_procs::block_age(b, cur.state, *mossiness, rng),
            }
            .map(|st| BInfo { state: st, ..cur.clone() })
            .or(Some(cur))
        }
        Proc::Blackstone => Some(BInfo {
            state: match pt {
                Some(t) => crate::structure_procs::blackstone_fast(b, t, cur.state),
                None => crate::structure_procs::blackstone(b, cur.state),
            },
            ..cur
        }),
        Proc::LavaSubmerged => {
            let w = r.get_p(cur.pos);
            if b.fluid(w) == 2 && !b.has_flag(cur.state, rc_data::flags::FULL_COLLISION) {
                Some(BInfo { state: b.default_state(b.k.lava), nbt: None, ..cur })
            } else {
                Some(cur)
            }
        }
    }
}

/// Box of the template frame whose image (mirror, rotation, pivot, then `pos`) is `bb`:
/// the inverse rotation around the same pivot, then the mirror (its own inverse).
pub(crate) fn local_box(bb: &BBox, pos: [i32; 3], s: &Placing) -> BBox {
    let back = match s.rot {
        Rot::Cw90 => Rot::Ccw90,
        Rot::Ccw90 => Rot::Cw90,
        r => r,
    };
    let un = |w: [i32; 3]| {
        let q = transform([w[0] - pos[0], w[1] - pos[1], w[2] - pos[2]], Mirror::None, back, s.pivot);
        match s.mirror {
            Mirror::LeftRight => [q[0], q[1], -q[2]],
            Mirror::FrontBack => [-q[0], q[1], q[2]],
            Mirror::None => q,
        }
    };
    BBox::new(un(bb.min), un(bb.max))
}

/// Processing of the palette blocks (§ 10.4).
#[allow(clippy::too_many_arguments)]
fn process(
    shared: &mut Option<&mut DecoRng>,
    r: &Region,
    b: &Blocks,
    seed: i64,
    pos: [i32; 3],
    reference: [i32; 3],
    s: &Placing,
    blocks: &[crate::template::TBlock],
    cats: [usize; 2],
    pt: Option<&ProcTables>,
) -> Vec<BInfo> {
    let whole = s.procs.iter().any(|p| matches!(p, Proc::Capped { .. }));
    let mut originals: Vec<[i32; 3]> = Vec::new();
    let mut out: Vec<BInfo> = Vec::new();
    let mut visit = |tb: &crate::template::TBlock| {
        let p = transform(tb.pos, s.mirror, s.rot, s.pivot);
        let w = [p[0] + pos[0], p[1] + pos[1], p[2] + pos[2]];
        if !whole
            && let Some(bb) = &s.bbox
            && !bb.contains(w)
        {
            return;
        }
        let mut cur = Some(BInfo { pos: w, state: tb.state, nbt: tb.nbt.clone() });
        for pr in s.procs.iter() {
            let Some(c) = cur.take() else { break };
            cur = process_one(shared, pr, r, b, reference, tb.pos, c, s, pt);
        }
        if let Some(c) = cur {
            out.push(c);
            originals.push(tb.pos);
        }
    };
    match &s.bbox {
        // Area brought back into the template frame (quarter-turn rotations and mirrors: aligned
        // box); each category is sorted by (y, x, z): layers in y, then range in x
        // by binary search, then test in z. Same visit order, same blocks as the area test.
        Some(bb) if !whole && crate::opt::process_box() && cats[0] <= cats[1] && cats[1] <= blocks.len() => {
            let l = local_box(bb, pos, s);
            for (a, e) in [(0, cats[0]), (cats[0], cats[1]), (cats[1], blocks.len())] {
                let mut i = a + blocks[a..e].partition_point(|t| t.pos[1] < l.min[1]);
                while i < e && blocks[i].pos[1] <= l.max[1] {
                    let y = blocks[i].pos[1];
                    let end = i + blocks[i..e].partition_point(|t| t.pos[1] <= y);
                    let layer = &blocks[i..end];
                    let x0 = layer.partition_point(|t| t.pos[0] < l.min[0]);
                    let x1 = layer.partition_point(|t| t.pos[0] <= l.max[0]).max(x0);
                    for tb in &layer[x0..x1] {
                        if tb.pos[2] >= l.min[2] && tb.pos[2] <= l.max[2] {
                            visit(tb);
                        }
                    }
                    i = end;
                }
            }
        }
        _ => blocks.iter().for_each(&mut visit),
    }
    for pr in s.procs.iter() {
        if let Proc::Capped { delegate, limit } = pr {
            if limit.max_value() == 0 || out.is_empty() {
                continue;
            }
            let mut base = DecoRng::legacy(seed);
            let fork = base.next_long();
            let mut rng = DecoRng::legacy(crate::rng::position_seed(pos[0], pos[1], pos[2]) ^ fork);
            let m = limit.sample(&mut rng).min(out.len() as i32);
            if m < 1 {
                continue;
            }
            let mut idx: Vec<usize> = (0..out.len()).collect();
            rng.shuffle(&mut idx);
            let mut replaced = 0;
            for i in idx {
                if replaced >= m {
                    break;
                }
                let cur = out[i].clone();
                if let Some(alt) = process_one(shared, delegate, r, b, reference, originals[i], cur.clone(), s, pt)
                    && alt != cur
                {
                    out[i] = alt;
                    replaced += 1;
                }
            }
        }
    }
    out
}

pub(crate) fn is_loot_container(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    matches!(n, "chest" | "trapped_chest" | "barrel" | "dispenser" | "dropper" | "hopper" | "crafter" | "decorated_pot")
        || n.ends_with("shulker_box")
}

/// Placement of a template (§ 10.3). `flags`: placement flags; `rng`: placement randomness (loot seeds).
#[allow(clippy::too_many_arguments)]
pub fn place_template(
    r: &mut Region,
    b: &Blocks,
    tr: &crate::template::StateTransforms,
    seed: i64,
    t: &Template,
    pos: [i32; 3],
    reference: [i32; 3],
    s: &Placing,
    rng: &mut DecoRng,
    flags: u32,
) -> bool {
    if t.palettes.is_empty() {
        return false;
    }
    let n = t.palettes.len() as i32;
    let pi = if s.shared { rng.next_int_bounded(n) } else { s.random(pos).next_int_bounded(n) } as usize;
    let blocks = &t.palettes[pi];
    if (blocks.is_empty() && t.entities.is_empty()) || t.size.iter().any(|&v| v < 1) {
        return false;
    }
    let pt = crate::opt::procs_tables().then_some(&tr.procs);
    let cats = t.cats.get(pi).copied().unwrap_or([blocks.len(); 2]);
    let infos = {
        let mut sh = if s.shared { Some(&mut *rng) } else { None };
        process(&mut sh, r, b, seed, pos, reference, s, blocks, cats, pt)
    };
    let mut to_fill: Vec<[i32; 3]> = Vec::new();
    let mut locked: Vec<[i32; 3]> = Vec::new();
    let set = crate::opt::locked_set();
    let mut locked_set: HashSet<[i32; 3]> = HashSet::new();
    let mut placed: Vec<[i32; 3]> = Vec::new();
    let waterlogged = b.prop("waterlogged");
    for bi in infos {
        if let Some(bb) = &s.bbox
            && !bb.contains(bi.pos)
        {
            continue;
        }
        let prev = if s.waterlog { Some(r.get_p(bi.pos)) } else { None };
        let state = tr.apply(bi.state, s.mirror, s.rot);
        if !r.set_p(bi.pos, state, flags) {
            continue;
        }
        placed.push(bi.pos);
        if let Some(n) = bi.nbt
            && b.facts(state).block_entity
        {
            let mut n = n;
            let loot = match pt {
                Some(t) => t.loot_container[b.block(state) as usize],
                None => is_loot_container(b.block_name(b.block(state))),
            };
            if loot && let Nbt::Compound(f) = &mut n
            {
                f.retain(|(k, _)| k != "LootTableSeed");
                f.push(("LootTableSeed".into(), Nbt::Long(rng.next_long())));
            }
            r.note_block_entity(bi.pos, BeData::Raw(n));
        }
        if let Some(prev) = prev {
            if b.is_source(state) {
                if set {
                    locked_set.insert(bi.pos);
                } else {
                    locked.push(bi.pos);
                }
            } else if let Some(wl) = waterlogged
                && b.has(state, wl)
            {
                if b.water_source(prev) && b.get_bool(state, wl) == Some(false) {
                    r.set_p(bi.pos, b.with_bool(state, wl, true), 3);
                }
                if !b.is_source(prev) {
                    to_fill.push(bi.pos);
                }
            }
        }
    }
    let mut filled = true;
    while filled && !to_fill.is_empty() {
        filled = false;
        to_fill.retain(|&p| {
            let cur = r.get_p(p);
            let mut source = b.is_source(cur);
            let mut water = b.water_source(cur);
            if !source {
                for d in [[0, 1, 0], [0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]] {
                    let q = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
                    let n = r.get_p(q);
                    let is_locked = if set { locked_set.contains(&q) } else { locked.contains(&q) };
                    if b.is_source(n) && !is_locked {
                        source = true;
                        water = b.water_source(n);
                    }
                    if source {
                        break;
                    }
                }
            }
            if source
                && let Some(wl) = waterlogged
                && b.has(cur, wl)
            {
                if water && b.get_bool(cur, wl) == Some(false) {
                    r.set_p(p, b.with_bool(cur, wl, true), 3);
                }
                filled = true;
                return false;
            }
            true
        });
    }
    if !s.known_shape {
        crate::shape_update::after_template(r, &placed);
    }
    if let Some(fin) = &s.entities {
        place_entities(r, t, pos, s, fin.as_deref());
    }
    true
}

/// Mobs of the game's pool templates (villages, outposts, bastions), finalised at placement;
/// the other entities (armor stands, cushions) are not mobs and stay as they are.
const FINALIZED_MOBS: [&str; 13] = [
    "minecraft:allay",
    "minecraft:camel",
    "minecraft:cat",
    "minecraft:cow",
    "minecraft:hoglin",
    "minecraft:horse",
    "minecraft:iron_golem",
    "minecraft:pig",
    "minecraft:piglin",
    "minecraft:piglin_brute",
    "minecraft:sheep",
    "minecraft:villager",
    "minecraft:zombie_villager",
];

/// Entities of the template (§ 10.3): position and look transformed, identifier removed; the
/// villagers take the biome's type when the placement prepares them.
fn place_entities(r: &mut Region, t: &Template, pos: [i32; 3], s: &Placing, villagers: Option<&[&'static str]>) {
    for e in &t.entities {
        let bp = crate::template::transform(e.block_pos, s.mirror, s.rot, s.pivot);
        let bp = [bp[0] + pos[0], bp[1] + pos[1], bp[2] + pos[2]];
        if let Some(bb) = &s.bbox
            && !bb.contains(bp)
        {
            continue;
        }
        let rp = crate::template::transform_f(e.pos, s.mirror, s.rot, s.pivot);
        let wp = [rp[0] + f64::from(pos[0]), rp[1] + f64::from(pos[1]), rp[2] + f64::from(pos[2])];
        let Nbt::Compound(mut f) = e.nbt.clone() else { continue };
        f.retain(|(k, _)| k != "UUID" && k != "Pos");
        f.push(("Pos".into(), Nbt::List(wp.iter().map(|v| Nbt::Double(*v)).collect())));
        if let Some(i) = f.iter().position(|(k, _)| k == "block_pos") {
            f[i].1 = Nbt::IntArray(bp.to_vec());
        }
        let (yaw, pitch) = match f.iter().find(|(k, _)| k == "Rotation").map(|(_, v)| v) {
            Some(Nbt::List(l)) if l.len() == 2 => match (&l[0], &l[1]) {
                (Nbt::Float(a), Nbt::Float(b)) => (*a, *b),
                _ => (0.0, 0.0),
            },
            _ => (0.0, 0.0),
        };
        let rotated = match s.rot {
            Rot::Cw180 => yaw + 180.0,
            Rot::Ccw90 => yaw + 270.0,
            Rot::Cw90 => yaw + 90.0,
            Rot::None => yaw,
        };
        let mirrored = match s.mirror {
            Mirror::LeftRight => -yaw,
            Mirror::FrontBack => 180.0 - yaw,
            Mirror::None => yaw,
        };
        let new_yaw = rotated + mirrored - yaw;
        f.retain(|(k, _)| k != "Rotation");
        f.push(("Rotation".into(), Nbt::List(vec![Nbt::Float(new_yaw), Nbt::Float(pitch)])));
        let id = f.iter().find(|(k, _)| k == "id").and_then(|(_, v)| v.as_str()).unwrap_or("").to_owned();
        if let Some(types) = villagers
            && matches!(id.as_str(), "minecraft:villager" | "minecraft:zombie_villager")
            && !f.iter().any(|(k, v)| k == "VillagerDataFinalized" && matches!(v, Nbt::Byte(1)))
        {
            let b = [wp[0].floor() as i32, wp[1].floor() as i32, wp[2].floor() as i32];
            let ty = types.get(r.biome(b[0], b[1], b[2]) as usize).copied().unwrap_or("minecraft:plains");
            let mut vd = match f.iter().find(|(k, _)| k == "VillagerData").map(|(_, v)| v.clone()) {
                Some(Nbt::Compound(v)) => v,
                _ => Vec::new(),
            };
            vd.retain(|(k, _)| k != "type");
            vd.push(("type".into(), Nbt::String(ty.into())));
            f.retain(|(k, _)| k != "VillagerData" && k != "VillagerDataFinalized");
            f.push(("VillagerData".into(), Nbt::Compound(vd)));
            f.push(("VillagerDataFinalized".into(), Nbt::Byte(1)));
        }
        // Finalising placement (pool elements): every mob goes through the game's finalisation
        // (variants and colours by biome, babies, bonus, hand); the server does it at
        // the first load of the entity, marker removed.
        if villagers.is_some() && FINALIZED_MOBS.contains(&id.as_str()) {
            f.push(("rc_finalize_structure".into(), Nbt::Byte(1)));
        }
        r.add_entity(wp[0].floor() as i32, wp[2].floor() as i32, Nbt::Compound(f));
    }
}

// ---------------------------------------------------------------- Placement per step

/// Placement of the structures of a step in the central chunk (§ 2.3, § 9.2).
pub fn place_step(
    cx: &mut Ctx,
    sg: &StructureGen,
    _t: &TerrainGenerator,
    refs: &[(usize, Vec<Start>)],
    step: usize,
    rng: &mut DecoRng,
    deco_seed: i64,
) {
    let (x0, z0) = (cx.r.cx * 16, cx.r.cz * 16);
    let zone = BBox::new([x0, cx.r.min_y + 1, z0], [x0 + 15, cx.r.max_y(), z0 + 15]);
    for (k, &s) in sg.by_step[step].iter().enumerate() {
        rng.set_feature_seed(deco_seed, k as i32, step as i32);
        let Some((_, starts)) = refs.iter().find(|(id, _)| *id == s) else { continue };
        for st in starts {
            let Some(first) = st.pieces.first() else { continue };
            let fb = first.bbox();
            let c = fb.center();
            let reference = [c[0], fb.min[1], c[2]];
            for p in &st.pieces {
                if p.bbox().intersects(&zone) {
                    place_piece(cx, sg, p, rng, zone, reference);
                }
            }
            if matches!(sg.defs[s].kind, crate::structure::Kind::DesertPyramid) {
                crate::structure_scattered::pyramid_after(cx.r, &st.pieces, zone);
            }
            if matches!(sg.defs[s].kind, crate::structure::Kind::Mansion) {
                crate::structure_mansion::after(cx.r, &st.pieces, zone);
            }
        }
    }
}

fn place_piece(cx: &mut Ctx, sg: &StructureGen, p: &Piece, rng: &mut DecoRng, zone: BBox, reference: [i32; 3]) {
    match p {
        Piece::Jigsaw(jp) => {
            place_element(cx, sg, &jp.element, jp, jp.pos, rng, zone, reference);
        }
        Piece::Coded { data, bbox, .. } => match &**data {
            CodedPiece::BuriedTreasure => buried_treasure(cx, *bbox, rng, zone),
            CodedPiece::Template(tp) => crate::structure_templ::place(cx, sg, tp, rng, zone, reference),
            CodedPiece::Scattered(sp) => crate::structure_scattered::place(cx, sg, sp, rng, zone),
            CodedPiece::Stronghold(sp) => crate::structure_stronghold::place(cx, sg, sp, *bbox, rng, zone),
            CodedPiece::Fortress(fp) => crate::structure_fortress::place(cx, sg, fp, *bbox, rng, zone),
            CodedPiece::Monument(m) => crate::structure_monument::place(cx, sg, m, *bbox, rng, zone),
            d => crate::structure_mineshaft::place(cx, sg, d, *bbox, rng, zone),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn place_element(
    cx: &mut Ctx,
    sg: &StructureGen,
    e: &Element,
    jp: &JigsawPiece,
    pos: [i32; 3],
    rng: &mut DecoRng,
    zone: BBox,
    reference: [i32; 3],
) -> bool {
    let b = cx.r.b;
    match e {
        Element::Single { processors, projection, legacy, liquid, procs: cell, .. } => {
            let Some(tp) = e.template(&sg.templates, b) else { return false };
            let mut pre: Vec<Proc> = Vec::new();
            if !legacy {
                pre.push(Proc::Ignore(b.block_id("minecraft:structure_block").into_iter().collect()));
            }
            pre.push(Proc::JigsawReplace);
            let mut post: Vec<Proc> = Vec::new();
            if *projection == Projection::TerrainMatching {
                post.push(Proc::Gravity { hm: Hm::WorldSurfaceWg, offset: -1 });
            }
            if *legacy {
                post.push(Proc::Ignore(
                    ["minecraft:structure_block", "minecraft:air"].iter().filter_map(|n| b.block_id(n)).collect(),
                ));
            }
            let procs = if crate::opt::procs_shared() {
                // List of the element analysed once (inline processors included), shared.
                ProcChain { pre, shared: cell.get_or_init(|| sg.procs.get(processors, b)).clone(), post }
            } else {
                pre.extend(sg.procs.get(processors, b).iter().cloned());
                pre.extend(post);
                pre.into()
            };
            let s = Placing {
                mirror: Mirror::None,
                rot: jp.rot,
                pivot: [0, 0, 0],
                bbox: Some(zone),
                procs,
                waterlog: liquid.unwrap_or(jp.waterlog),
                seed: None,
                shared: false,
                known_shape: true,
                entities: Some(Some(sg.villager_types.clone())),
            };
            place_template(cx.r, b, &sg.templates.transforms, sg.seed, &tp, pos, reference, &s, rng, 18)
        }
        Element::List { elements, .. } => {
            for el in elements {
                if !place_element(cx, sg, el, jp, pos, rng, zone, reference) {
                    return false;
                }
            }
            true
        }
        Element::Feature { feature, .. } => match cx.d.reg.placed_id(feature) {
            Some(id) => crate::placement::place(cx, id, rng, pos, None),
            None => false,
        },
        Element::Empty => true,
    }
}

// ---------------------------------------------------------------- Buried treasure (§ 8.14)

fn buried_treasure(cx: &mut Ctx, bbox: BBox, rng: &mut DecoRng, zone: BBox) {
    let r = &mut *cx.r;
    let b = r.b;
    let (x, z) = (bbox.min[0], bbox.min[2]);
    if !zone.contains([x, zone.min[1], z]) {
        return;
    }
    let names = ["sandstone", "stone", "andesite", "granite", "diorite"];
    let solid: Vec<u16> = names.iter().filter_map(|n| b.block_id(n)).collect();
    let sand = b.default_state(b.k.sand);
    let mut y = r.height_at(Hm::OceanFloorWg, x, z);
    let liquid = |s: State| b.is(s, b.k.water) || b.is(s, b.k.lava);
    while y > r.min_y {
        let cur = r.get(x, y, z);
        let below = r.get(x, y - 1, z);
        if solid.contains(&b.block(below)) {
            let soft = if b.is_air(cur) || liquid(cur) { sand } else { cur };
            for (i, d) in [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]].iter().enumerate() {
                let q = [x + d[0], y + d[1], z + d[2]];
                let v = r.get_p(q);
                if b.is_air(v) || liquid(v) {
                    let under = r.get(q[0], q[1] - 1, q[2]);
                    let st = if (b.is_air(under) || liquid(under)) && i != 1 { below } else { soft };
                    r.set_p(q, st, 3);
                }
            }
            if zone.contains([x, y, z]) && !b.is(r.get(x, y, z), b.k.chest) {
                let chest = crate::feature_basic::reorient_chest(r, [x, y, z], b.default_state(b.k.chest));
                r.set(x, y, z, chest, 2);
                let seed = rng.next_long();
                r.note_block_entity([x, y, z], BeData::Loot { table: "minecraft:chests/buried_treasure", seed });
            }
            return;
        }
        y -= 1;
    }
}
