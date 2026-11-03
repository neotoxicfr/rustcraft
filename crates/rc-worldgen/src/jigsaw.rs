//! Jigsaw structures (`generation-decoration.md` § 8.1): pools, elements, aliases, assembly of the
//! pieces (free region, priority queue, village enlargement trick).

use hashbrown::HashMap;
use std::borrow::Cow;
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use crate::Error;
use crate::data::full_id;
use crate::decor_blocks::{Blocks, Dir};
use crate::decor_data::DecorData;
use crate::decor_pred::{HeightP, Weighted};
use crate::decor_rng::DecoRng;
use crate::json::Json;
use crate::structure::{GenStart, Piece};
use crate::template::{BBox, Rot, Settings, Template, Templates, transform};

/// Work counters of the assemblies run on the current thread, read by the benches, counted only
/// while a bench asks for them (the timed passes stay free of them). Tests only: the game build
/// carries none of them.
#[cfg(test)]
pub(crate) mod work {
    use std::cell::Cell;

    #[derive(Debug, Default, Clone, Copy)]
    pub(crate) struct AssemblyWork {
        /// Free space tests.
        pub fits_calls: u64,
        /// Boxes already placed, summed over the free space tests (average = this / calls).
        pub placed_boxes_scanned: u64,
        /// Placed boxes actually compared with a trial box (the walk stops at the first hit).
        pub boxes_tested: u64,
        /// Most boxes placed in one free space at a test.
        pub placed_boxes_max: usize,
        /// Ground height reads (terrain height without decoration) and their time.
        pub ground_reads: u64,
        pub ground_nanos: u64,
        /// Pool lookups by identifier.
        pub pool_lookups: u64,
        /// Candidate elements met at a junction (each one tried over its four rotations).
        pub element_trials: u64,
        /// Of those, elements that already failed at the same junction: their draws replayed
        /// without a trial (failed-element memo on).
        pub replayed_failures: u64,
    }

    thread_local! {
        static COUNTING: Cell<bool> = const { Cell::new(false) };
        static WORK: Cell<AssemblyWork> = Cell::new(AssemblyWork::default());
    }

    /// Starts or stops the counting on the current thread.
    pub(crate) fn count_here(on: bool) {
        COUNTING.with(|c| c.set(on));
    }

    pub(crate) fn add(f: impl FnOnce(&mut AssemblyWork)) {
        if !COUNTING.with(Cell::get) {
            return;
        }
        WORK.with(|c| {
            let mut w = c.get();
            f(&mut w);
            c.set(w);
        });
    }

    /// Counters since the last call, reset.
    pub(crate) fn take() -> AssemblyWork {
        WORK.with(Cell::take)
    }
}

#[derive(Debug, Clone)]
pub enum Alias {
    Direct(String, String),
    Random(String, Weighted<String>),
    Group(Weighted<Vec<Alias>>),
}

fn parse_alias(j: &Json) -> Option<Alias> {
    let s = |k: &str| j.get(k).and_then(Json::as_str).map(full_id);
    Some(match j.get("type")?.as_str()? {
        "minecraft:direct" => Alias::Direct(s("alias")?, s("target")?),
        "minecraft:random" => {
            let items = j
                .get("targets")?
                .as_array()?
                .iter()
                .filter_map(|t| Some((full_id(t.get("data")?.as_str()?), t.get("weight")?.as_i32()?)))
                .collect();
            Alias::Random(s("alias")?, Weighted::new(items))
        }
        "minecraft:random_group" => {
            let items = j
                .get("groups")?
                .as_array()?
                .iter()
                .filter_map(|g| {
                    let v: Vec<Alias> = g.get("data")?.as_array()?.iter().filter_map(parse_alias).collect();
                    Some((v, g.get("weight")?.as_i32()?))
                })
                .collect();
            Alias::Group(Weighted::new(items))
        }
        _ => return None,
    })
}

/// Settings of a `jigsaw` structure.
#[derive(Debug)]
pub struct JigsawCfg {
    pub start_pool: String,
    pub start_jigsaw_name: Option<String>,
    pub size: i32,
    pub start_height: HeightP,
    pub expansion_hack: bool,
    /// `Some(seabed)`: projection of the start onto the map (`OCEAN_FLOOR_WG` → true).
    pub project: Option<bool>,
    pub max_h: i32,
    pub max_v: Option<i32>,
    pub aliases: Vec<Alias>,
    pub pad_bottom: i32,
    pub pad_top: i32,
    pub waterlog: bool,
}

impl JigsawCfg {
    pub fn parse(j: &Json) -> Result<JigsawCfg, Error> {
        let (max_h, max_v) = match j.get("max_distance_from_center") {
            Some(Json::Obj(m)) => (
                m.get("horizontal").and_then(Json::as_i32).unwrap_or(80),
                Some(m.get("vertical").and_then(Json::as_i32).unwrap_or(4064)),
            ),
            Some(v) => (v.as_i32().unwrap_or(80), v.as_i32()),
            None => (80, None),
        };
        let (pad_bottom, pad_top) = match j.get("dimension_padding") {
            Some(Json::Obj(m)) => {
                (m.get("bottom").and_then(Json::as_i32).unwrap_or(0), m.get("top").and_then(Json::as_i32).unwrap_or(0))
            }
            Some(v) => {
                let n = v.as_i32().unwrap_or(0);
                (n, n)
            }
            None => (0, 0),
        };
        Ok(JigsawCfg {
            start_pool: full_id(j.get("start_pool").and_then(Json::as_str).unwrap_or("minecraft:empty")),
            start_jigsaw_name: j.get("start_jigsaw_name").and_then(Json::as_str).map(full_id),
            size: j.get("size").and_then(Json::as_i32).unwrap_or(0),
            start_height: HeightP::parse(j.get("start_height").unwrap_or(&Json::Null))
                .unwrap_or(HeightP::Const(crate::decor_pred::Anchor::Absolute(0))),
            expansion_hack: j.get("use_expansion_hack").and_then(Json::as_bool).unwrap_or(false),
            project: j.get("project_start_to_heightmap").and_then(Json::as_str).map(|s| s == "OCEAN_FLOOR_WG"),
            max_h,
            max_v,
            aliases: j
                .get("pool_aliases")
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(parse_alias)
                .collect(),
            pad_bottom,
            pad_top,
            waterlog: j.get("liquid_settings").and_then(Json::as_str) != Some("ignore_waterlogging"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Rigid,
    TerrainMatching,
}

/// Pool element.
#[derive(Debug)]
pub enum Element {
    Single {
        location: String,
        processors: Json,
        projection: Projection,
        legacy: bool,
        liquid: Option<bool>,
        /// Template resolved at first need (`RUSTCRAFT_OPT_ELEM_ONCE`), for the templates of the
        /// structure generation that owns this pool.
        tpl: OnceLock<Option<Arc<Template>>>,
        /// Junctions and boxes per rotation at zero position (`RUSTCRAFT_OPT_JIGSAW_PRECALC`).
        pre: OnceLock<Option<Box<PrecomputedRotations>>>,
        /// Processors analysed once and shared by the placements (`RUSTCRAFT_OPT_PROCS_SHARED`).
        procs: OnceLock<Arc<Vec<crate::structure_place::Proc>>>,
    },
    List {
        elements: Vec<Arc<Element>>,
        projection: Projection,
    },
    Feature {
        feature: String,
        projection: Projection,
    },
    Empty,
}

fn projection(j: &Json) -> Projection {
    if j.get("projection").and_then(Json::as_str) == Some("terrain_matching") {
        Projection::TerrainMatching
    } else {
        Projection::Rigid
    }
}

fn parse_element(j: &Json) -> Element {
    let p = projection(j);
    match j.get("element_type").and_then(Json::as_str).unwrap_or("") {
        "minecraft:single_pool_element" | "minecraft:legacy_single_pool_element" => Element::Single {
            location: full_id(j.get("location").and_then(Json::as_str).unwrap_or("")),
            processors: j.get("processors").cloned().unwrap_or(Json::Null),
            projection: p,
            legacy: j.get("element_type").and_then(Json::as_str) == Some("minecraft:legacy_single_pool_element"),
            liquid: j.get("override_liquid_settings").and_then(Json::as_str).map(|s| s != "ignore_waterlogging"),
            tpl: OnceLock::new(),
            pre: OnceLock::new(),
            procs: OnceLock::new(),
        },
        "minecraft:list_pool_element" => Element::List {
            elements: j
                .get("elements")
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .map(|e| Arc::new(parse_element(e)))
                .collect(),
            projection: p,
        },
        "minecraft:feature_pool_element" => {
            Element::Feature { feature: full_id(j.get("feature").and_then(Json::as_str).unwrap_or("")), projection: p }
        }
        _ => Element::Empty,
    }
}

/// Junction block at world position.
#[derive(Debug, Clone)]
pub struct JunctionBlock {
    pub pos: [i32; 3],
    pub front: Dir,
    pub top: Dir,
    pub name: Arc<str>,
    pub pool: Arc<str>,
    pub target: Arc<str>,
    pub rollable: bool,
    pub placement_priority: i32,
    pub selection_priority: i32,
}

impl JunctionBlock {
    /// Junction of the rotated template, at zero position.
    fn turned(j: &crate::template::Jigsaw, rot: Rot) -> JunctionBlock {
        JunctionBlock {
            pos: transform(j.pos, crate::template::Mirror::None, rot, [0, 0, 0]),
            front: rot.dir(j.front),
            top: rot.dir(j.top),
            name: j.name.clone(),
            pool: j.pool.clone(),
            target: j.target.clone(),
            rollable: j.rollable,
            placement_priority: j.placement_priority,
            selection_priority: j.selection_priority,
        }
    }
}

/// Single junction of an "in-place structure" element (nameless: attaches to any target).
static FEATURE_JIGSAW: LazyLock<[JunctionBlock; 1]> = LazyLock::new(|| {
    [JunctionBlock {
        pos: [0, 0, 0],
        front: Dir::Down,
        top: Dir::South,
        name: "".into(),
        pool: "minecraft:empty".into(),
        target: "minecraft:empty".into(),
        rollable: true,
        placement_priority: 0,
        selection_priority: 0,
    }]
});

/// Precomputation of a "single template" element: per rotation, box and junctions of each
/// palette at zero position.
#[derive(Debug)]
pub struct PrecomputedRotations {
    bbox0: [BBox; 4],
    jigs: [Vec<Vec<JunctionBlock>>; 4],
}

impl PrecomputedRotations {
    fn new(tp: &Template) -> PrecomputedRotations {
        PrecomputedRotations {
            bbox0: Rot::ALL.map(|r| tp.bbox([0, 0, 0], &Settings::rot(r))),
            jigs: Rot::ALL.map(|r| {
                tp.jigsaws.iter().map(|pal| pal.iter().map(|j| JunctionBlock::turned(j, r)).collect()).collect()
            }),
        }
    }
}

/// Junctions of a piece in trial order: base list, offset, order (`None` = list), the order
/// owned or held in a buffer kept by the caller.
pub(crate) struct JunctionView<'a, 'o> {
    list: Cow<'a, [JunctionBlock]>,
    off: [i32; 3],
    order: Option<Cow<'o, [u32]>>,
}

impl JunctionView<'_, '_> {
    /// (junction, world position).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&JunctionBlock, [i32; 3])> {
        (0..self.len()).map(move |k| {
            let j = &self.list[self.order.as_ref().map_or(k, |o| o[k] as usize)];
            (j, [j.pos[0] + self.off[0], j.pos[1] + self.off[1], j.pos[2] + self.off[2]])
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.order.as_ref().map_or(self.list.len(), |o| o.len())
    }
}

/// Trial order of a junction list by index: shuffled then sorted (stable) by decreasing
/// selection priority when `mix`, list order otherwise. The shuffle depends only on the length:
/// same draws, same permutation as on the list itself.
fn fill_trial_order(list: &[JunctionBlock], mix: bool, rng: &mut DecoRng, order: &mut Vec<u32>) {
    order.clear();
    order.extend(0..list.len() as u32);
    if mix {
        rng.shuffle(order);
        order.sort_by_key(|&i| std::cmp::Reverse(list[i as usize].selection_priority));
    }
}

/// Palette of a piece placed at `pos` (position seed), among `n`.
fn palette_at(pos: [i32; 3], n: usize) -> usize {
    DecoRng::legacy(crate::rng::position_seed(pos[0], pos[1], pos[2])).next_int_bounded(n as i32) as usize
}

impl Element {
    pub fn projection(&self) -> Projection {
        match self {
            Element::Single { projection, .. }
            | Element::List { projection, .. }
            | Element::Feature { projection, .. } => *projection,
            Element::Empty => Projection::Rigid,
        }
    }

    /// Template of a "single template" element; `t` is always the templates of the structure
    /// generation that owns the element's pool (the cell serves only them).
    pub(crate) fn template(&self, t: &Templates, b: &Blocks) -> Option<Arc<Template>> {
        match self {
            Element::Single { location, tpl, .. } if crate::opt::elem_once() => {
                tpl.get_or_init(|| t.get(location, b)).clone()
            }
            Element::Single { location, .. } => t.get(location, b),
            _ => None,
        }
    }

    fn pre(&self, t: &Templates, b: &Blocks) -> Option<&PrecomputedRotations> {
        match self {
            Element::Single { pre, .. } => {
                pre.get_or_init(|| self.template(t, b).map(|tp| Box::new(PrecomputedRotations::new(&tp)))).as_deref()
            }
            _ => None,
        }
    }

    /// Box at zero position through the precomputation (same value as [`Element::bbox`] at (0, 0, 0)).
    fn bbox0(&self, t: &Templates, b: &Blocks, rot: Rot) -> BBox {
        let zero = BBox::new([0, 0, 0], [0, 0, 0]);
        match self {
            Element::Single { .. } => self.pre(t, b).map_or(zero, |p| p.bbox0[rot as usize]),
            Element::List { elements, .. } => {
                let mut out: Option<BBox> = None;
                for e in elements.iter().filter(|e| !matches!(***e, Element::Empty)) {
                    let bb = e.bbox0(t, b, rot);
                    match &mut out {
                        Some(o) => o.encapsulate(&bb),
                        None => out = Some(bb),
                    }
                }
                out.unwrap_or(zero)
            }
            Element::Feature { .. } | Element::Empty => zero,
        }
    }

    pub fn bbox(&self, t: &Templates, b: &Blocks, pos: [i32; 3], rot: Rot) -> BBox {
        if crate::opt::jigsaw_precalc() {
            return self.bbox0(t, b, rot).moved(pos);
        }
        self.bbox_old(t, b, pos, rot)
    }

    fn bbox_old(&self, t: &Templates, b: &Blocks, pos: [i32; 3], rot: Rot) -> BBox {
        match self {
            Element::Single { .. } => match self.template(t, b) {
                Some(tp) => tp.bbox(pos, &Settings::rot(rot)),
                None => BBox::new(pos, pos),
            },
            Element::List { elements, .. } => {
                let mut out: Option<BBox> = None;
                for e in elements.iter().filter(|e| !matches!(***e, Element::Empty)) {
                    let bb = e.bbox_old(t, b, pos, rot);
                    match &mut out {
                        Some(o) => o.encapsulate(&bb),
                        None => out = Some(bb),
                    }
                }
                out.unwrap_or_else(|| BBox::new(pos, pos))
            }
            Element::Feature { .. } | Element::Empty => BBox::new(pos, pos),
        }
    }

    /// Shuffled junctions (§ 8.1.1): palette of the position, transformed, shuffled then
    /// sorted (stable) by decreasing selection priority.
    pub fn shuffled_jigsaws(
        &self,
        t: &Templates,
        b: &Blocks,
        pos: [i32; 3],
        rot: Rot,
        rng: &mut DecoRng,
    ) -> Vec<JunctionBlock> {
        match self {
            Element::Single { .. } => {
                let Some(tp) = self.template(t, b) else { return Vec::new() };
                if tp.palettes.is_empty() {
                    return Vec::new();
                }
                let pi = palette_at(pos, tp.palettes.len());
                let mut v: Vec<JunctionBlock> = tp.jigsaws[pi]
                    .iter()
                    .map(|j| {
                        let w = JunctionBlock::turned(j, rot);
                        JunctionBlock { pos: [w.pos[0] + pos[0], w.pos[1] + pos[1], w.pos[2] + pos[2]], ..w }
                    })
                    .collect();
                rng.shuffle(&mut v);
                v.sort_by_key(|j| std::cmp::Reverse(j.selection_priority));
                v
            }
            Element::List { elements, .. } => {
                elements.first().map(|e| e.shuffled_jigsaws(t, b, pos, rot, rng)).unwrap_or_default()
            }
            Element::Feature { .. } => vec![JunctionBlock { pos, ..FEATURE_JIGSAW[0].clone() }],
            Element::Empty => Vec::new(),
        }
    }

    /// Junctions before shuffling at zero position, offset, and whether to shuffle and sort them.
    fn base_list<'a>(
        &'a self,
        t: &Templates,
        b: &Blocks,
        pos: [i32; 3],
        rot: Rot,
        precalc: bool,
    ) -> (Cow<'a, [JunctionBlock]>, [i32; 3], bool) {
        let none = (Cow::Borrowed(&[][..]), pos, false);
        match self {
            Element::Single { .. } if precalc => match self.pre(t, b) {
                Some(p) if !p.jigs[rot as usize].is_empty() => {
                    let pal = &p.jigs[rot as usize];
                    (Cow::Borrowed(pal[palette_at(pos, pal.len())].as_slice()), pos, true)
                }
                _ => none,
            },
            Element::Single { .. } => match self.template(t, b) {
                Some(tp) if !tp.palettes.is_empty() => {
                    let pi = palette_at(pos, tp.palettes.len());
                    (Cow::Owned(tp.jigsaws[pi].iter().map(|j| JunctionBlock::turned(j, rot)).collect()), pos, true)
                }
                _ => none,
            },
            Element::List { elements, .. } => match elements.first() {
                Some(e) => e.base_list(t, b, pos, rot, precalc),
                None => none,
            },
            Element::Feature { .. } => (Cow::Borrowed(&FEATURE_JIGSAW[..]), pos, false),
            Element::Empty => none,
        }
    }

    /// Junctions in trial order (same draws as [`Element::shuffled_jigsaws`]): precomputed
    /// lists (`precalc`), order by indices without copy (`idx`), written into `order_buffer`
    /// when the caller keeps one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn jigsaw_view<'a, 'o>(
        &'a self,
        t: &Templates,
        b: &Blocks,
        pos: [i32; 3],
        rot: Rot,
        rng: &mut DecoRng,
        precalc: bool,
        idx: bool,
        order_buffer: Option<&'o mut Vec<u32>>,
    ) -> JunctionView<'a, 'o> {
        if !precalc && !idx {
            return JunctionView {
                list: Cow::Owned(self.shuffled_jigsaws(t, b, pos, rot, rng)),
                off: [0; 3],
                order: None,
            };
        }
        let (list, off, mix) = self.base_list(t, b, pos, rot, precalc);
        if idx {
            let order = match order_buffer {
                Some(buffer) => {
                    fill_trial_order(&list, mix, rng, buffer);
                    let kept: &'o [u32] = buffer;
                    Cow::Borrowed(kept)
                }
                None => {
                    let mut order = Vec::new();
                    fill_trial_order(&list, mix, rng, &mut order);
                    Cow::Owned(order)
                }
            };
            return JunctionView { list, off, order: Some(order) };
        }
        let mut v: Vec<JunctionBlock> = list
            .iter()
            .map(|j| JunctionBlock { pos: [j.pos[0] + off[0], j.pos[1] + off[1], j.pos[2] + off[2]], ..j.clone() })
            .collect();
        if mix {
            rng.shuffle(&mut v);
            v.sort_by_key(|j| std::cmp::Reverse(j.selection_priority));
        }
        JunctionView { list: Cow::Owned(v), off: [0; 3], order: None }
    }
}

/// Template pool (`worldgen/template_pool`).
#[derive(Debug)]
pub struct Pool {
    pub fallback: String,
    /// Unfolded list (each element repeated `weight` times).
    pub templates: Vec<Arc<Element>>,
    max_size: Mutex<Option<i32>>,
    max_once: OnceLock<i32>,
}

/// Pools of the game, read on demand.
#[derive(Debug)]
pub struct Pools {
    /// Frozen table: text of the pool and pool analysed at first need (`RUSTCRAFT_OPT_POOLS_TABLE`;
    /// `None` = text that does not parse, like a pool absent at generator start).
    raw: HashMap<String, (PoolText, PoolCell)>,
    /// Old path: cache under lock, absent identifiers included.
    cache: Mutex<HashMap<String, Option<Arc<Pool>>>>,
}

/// Pool analysed once, at its first lookup.
type PoolCell = OnceLock<Option<Arc<Pool>>>;

/// Text of a pool: parsed at generator start, or its file bytes, parsed at its first lookup
/// (`RUSTCRAFT_OPT_LAZY_POOLS`: a world only ever looks up the pools of the structures it meets).
#[derive(Debug)]
enum PoolText {
    Parsed(Json),
    Bytes(Arc<[u8]>),
}

impl PoolText {
    fn json(&self) -> Option<Cow<'_, Json>> {
        match self {
            PoolText::Parsed(j) => Some(Cow::Borrowed(j)),
            PoolText::Bytes(b) => std::str::from_utf8(b).ok().and_then(|t| crate::json::parse(t).ok()).map(Cow::Owned),
        }
    }

    fn pool(&self) -> Option<Arc<Pool>> {
        self.json().map(|j| parse_pool(&j))
    }
}

fn parse_pool(j: &Json) -> Arc<Pool> {
    let mut templates = Vec::new();
    for e in j.get("elements").and_then(Json::as_array).unwrap_or(&[]) {
        let el = Arc::new(parse_element(e.get("element").unwrap_or(&Json::Null)));
        let w = e.get("weight").and_then(Json::as_i32).unwrap_or(1);
        for _ in 0..w {
            templates.push(el.clone());
        }
    }
    Arc::new(Pool {
        fallback: full_id(j.get("fallback").and_then(Json::as_str).unwrap_or("minecraft:empty")),
        templates,
        max_size: Mutex::new(None),
        max_once: OnceLock::new(),
    })
}

impl Pools {
    pub fn new(d: &DecorData, _b: &Blocks) -> Pools {
        Pools::read(d, crate::opt::lazy_pools())
    }

    /// Pools of the data, their text parsed now or at first lookup (`lazy`).
    pub(crate) fn read(d: &DecorData, lazy: bool) -> Pools {
        let raw = if lazy {
            let files = d.files_of("template_pool").into_iter();
            files.map(|(id, bytes)| (id, (PoolText::Bytes(bytes), OnceLock::new()))).collect()
        } else {
            let mut raw = HashMap::new();
            for id in d.ids("template_pool") {
                if let Ok(j) = d.json("template_pool", &id) {
                    raw.insert(id, (PoolText::Parsed(j), OnceLock::new()));
                }
            }
            raw
        };
        Pools { raw, cache: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, id: &str) -> Option<Arc<Pool>> {
        #[cfg(test)]
        work::add(|w| w.pool_lookups += 1);
        if crate::opt::pools_table() {
            let (text, cell) = self.raw.get(id)?;
            return cell.get_or_init(|| text.pool()).clone();
        }
        if let Some(p) = self.cache.lock().ok()?.get(id) {
            return p.clone();
        }
        let p = self.raw.get(id).and_then(|(text, _)| text.pool());
        if let Ok(mut c) = self.cache.lock() {
            c.insert(id.to_owned(), p.clone());
        }
        p
    }
}

impl Pool {
    fn max_size(&self, t: &Templates, b: &Blocks) -> i32 {
        if crate::opt::pools_table() {
            return *self.max_once.get_or_init(|| self.max_size_compute(t, b));
        }
        if let Ok(m) = self.max_size.lock()
            && let Some(v) = *m
        {
            return v;
        }
        let v = self.max_size_compute(t, b);
        if let Ok(mut m) = self.max_size.lock() {
            *m = Some(v);
        }
        v
    }

    fn max_size_compute(&self, t: &Templates, b: &Blocks) -> i32 {
        self.templates
            .iter()
            .filter(|e| !matches!(***e, Element::Empty))
            .map(|e| e.bbox(t, b, [0, 0, 0], Rot::None).span(1))
            .max()
            .unwrap_or(0)
    }
}

/// Recorded junction (for terrain adaptation).
#[derive(Debug, Clone, Copy)]
pub struct Junction {
    pub x: i32,
    pub ground_y: i32,
    pub z: i32,
    pub dy: i32,
    pub projection: Projection,
}

/// Jigsaw piece.
#[derive(Debug, Clone)]
pub struct JigsawPiece {
    pub element: Arc<Element>,
    pub pos: [i32; 3],
    pub ground_delta: i32,
    pub rot: Rot,
    pub bbox: BBox,
    pub junctions: Vec<Junction>,
    pub waterlog: bool,
}

/// Resolution of the pool aliases (§ 8.1.2).
fn resolve_aliases(cfg: &JigsawCfg, seed: i64, p: [i32; 3]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if cfg.aliases.is_empty() {
        return out;
    }
    let mut base = DecoRng::legacy(seed);
    let fork = base.next_long();
    let mut r = DecoRng::legacy(crate::rng::position_seed(p[0], p[1], p[2]) ^ fork);
    fn apply(a: &Alias, r: &mut DecoRng, out: &mut HashMap<String, String>) {
        match a {
            Alias::Direct(k, v) => {
                out.insert(k.clone(), v.clone());
            }
            Alias::Random(k, w) => {
                if let Some(v) = w.pick(r) {
                    out.insert(k.clone(), v.clone());
                }
            }
            Alias::Group(w) => {
                if let Some(g) = w.pick(r) {
                    for a in g {
                        apply(a, r, out);
                    }
                }
            }
        }
    }
    for a in &cfg.aliases {
        apply(a, &mut r, &mut out);
    }
    out
}

/// Paths of the assembly, read once per start (each one behind its `RUSTCRAFT_OPT_*` switch).
#[derive(Debug, Clone, Copy)]
struct Paths {
    precalc: bool,
    idx: bool,
    failed_element_memo: bool,
    expand_memo: bool,
    recent_first: bool,
    spatial_grid: bool,
    reused_buffers: bool,
}

impl Paths {
    fn read() -> Paths {
        use crate::opt;
        Paths {
            precalc: opt::jigsaw_precalc(),
            idx: opt::jigsaw_idx(),
            failed_element_memo: opt::jigsaw_failed_element_memo(),
            expand_memo: opt::jigsaw_expand_memo(),
            recent_first: opt::jigsaw_recent_first(),
            spatial_grid: opt::jigsaw_spatial_grid(),
            reused_buffers: opt::jigsaw_reused_buffers(),
        }
    }
}

/// Free volume: real cuboid minus removed boxes.
#[derive(Debug, Clone)]
struct Free {
    lo: [i32; 3],
    /// Exclusive upper bounds.
    hi: [i32; 3],
    removed: Vec<BBox>,
    /// Removed boxes by column cell, once they are many (`JIGSAW_SPATIAL_GRID`).
    cells: Option<ColumnCells>,
}

/// Removed boxes past which a free space indexes them by column cell.
const CELLS_FROM: usize = 100;

impl Free {
    fn new(lo: [i32; 3], hi: [i32; 3], removed: Vec<BBox>) -> Free {
        Free { lo, hi, removed, cells: None }
    }

    fn of_box(b: &BBox) -> Free {
        Free::new(b.min, [b.max[0] + 1, b.max[1] + 1, b.max[2] + 1], Vec::new())
    }

    /// The box shrunk by 0.25 per face is entirely free.
    fn fits(&self, b: &BBox, paths: Paths) -> bool {
        #[cfg(test)]
        work::add(|w| {
            w.fits_calls += 1;
            w.placed_boxes_scanned += self.removed.len() as u64;
            w.placed_boxes_max = w.placed_boxes_max.max(self.removed.len());
        });
        (0..3).all(|i| b.min[i] >= self.lo[i] && b.max[i] < self.hi[i]) && !self.meets_removed(b, paths)
    }

    fn meets_removed(&self, b: &BBox, paths: Paths) -> bool {
        let meets = |r: &BBox| {
            #[cfg(test)]
            work::add(|w| w.boxes_tested += 1);
            r.intersects(b)
        };
        if let Some(cells) = &self.cells {
            return cells.near(b).any(|i| meets(&self.removed[i]));
        }
        if paths.recent_first {
            // A trial that fails usually meets a box placed lately (its neighbours are the last
            // pieces): from the most recent box, the walk stops sooner.
            self.removed.iter().rev().any(meets)
        } else {
            self.removed.iter().any(meets)
        }
    }

    /// Takes the box of a placed piece out of the free space.
    fn remove(&mut self, b: BBox, paths: Paths) {
        self.removed.push(b);
        match &mut self.cells {
            Some(cells) => cells.add(self.removed.len() - 1, &b),
            None if paths.spatial_grid && self.removed.len() > CELLS_FROM => {
                self.cells = Some(ColumnCells::new(self.lo, self.hi, &self.removed));
            }
            None => {}
        }
    }
}

/// Removed boxes of a free space by column cell of 16 × 16 blocks over its bounds (indices into
/// the removed list): a test only meets the boxes of the cells it covers.
#[derive(Debug, Clone)]
struct ColumnCells {
    /// Low corner (x, z) of the grid and its size in cells.
    x0: i32,
    z0: i32,
    nx: i32,
    nz: i32,
    cells: Vec<Vec<u32>>,
}

impl ColumnCells {
    fn new(lo: [i32; 3], hi: [i32; 3], removed: &[BBox]) -> ColumnCells {
        let (nx, nz) = (((hi[0] - lo[0] + 15) >> 4).max(1), ((hi[2] - lo[2] + 15) >> 4).max(1));
        let mut grid = ColumnCells { x0: lo[0], z0: lo[2], nx, nz, cells: vec![Vec::new(); (nx * nz) as usize] };
        for (i, b) in removed.iter().enumerate() {
            grid.add(i, b);
        }
        grid
    }

    /// Cells covered by the box, clipped to the grid. A box part outside the grid never meets a
    /// tested box: that one lies inside the bounds of the free space.
    fn span(&self, b: &BBox) -> (std::ops::Range<i32>, std::ops::Range<i32>) {
        let x = ((b.min[0] - self.x0) >> 4).max(0)..(((b.max[0] - self.x0) >> 4) + 1).min(self.nx);
        let z = ((b.min[2] - self.z0) >> 4).max(0)..(((b.max[2] - self.z0) >> 4) + 1).min(self.nz);
        (x, z)
    }

    fn add(&mut self, i: usize, b: &BBox) {
        let (xs, zs) = self.span(b);
        for z in zs {
            for x in xs.clone() {
                self.cells[(z * self.nx + x) as usize].push(i as u32);
            }
        }
    }

    /// Removed boxes that share a cell with `b` (a box spread over several cells comes once per
    /// cell).
    fn near(&self, b: &BBox) -> impl Iterator<Item = usize> + '_ {
        let (xs, zs) = self.span(b);
        zs.flat_map(move |z| xs.clone().map(move |x| (z * self.nx + x) as usize))
            .flat_map(|c| self.cells[c].iter().map(|&i| i as usize))
    }
}

/// Start of a jigsaw structure (§ 8.1).
pub fn generate(g: &mut GenStart, cfg: &JigsawCfg) -> Option<([i32; 3], Vec<Piece>)> {
    let h = cfg.start_height.sample(&mut g.rng, g.hctx);
    generate_from(g, cfg, [16 * g.cx, h, 16 * g.cz])
}

/// Assembly from a given start position (jigsaw block, `place jigsaw`): no height drawn.
pub fn generate_from(g: &mut GenStart, cfg: &JigsawCfg, p: [i32; 3]) -> Option<([i32; 3], Vec<Piece>)> {
    let sg = g.sg;
    let (t, b) = (&*sg.templates, g.b);
    let aliases = resolve_aliases(cfg, sg.seed, p);
    let paths = Paths::read();
    let rot = Rot::from_index(g.rng.next_int_bounded(4));
    let pool = sg.pools.get(alias(&aliases, &cfg.start_pool))?;
    let e = if pool.templates.is_empty() {
        return None;
    } else {
        pool.templates[g.rng.next_int_bounded(pool.templates.len() as i32) as usize].clone()
    };
    if matches!(*e, Element::Empty) {
        return None;
    }
    let anchor = match &cfg.start_jigsaw_name {
        Some(name) => {
            let v = e.jigsaw_view(t, b, p, rot, &mut g.rng, paths.precalc, paths.idx, None);
            v.iter().find(|(j, _)| *j.name == **name)?.1
        }
        None => p,
    };
    let local = [anchor[0] - p[0], anchor[1] - p[1], anchor[2] - p[2]];
    let pos = [p[0] - local[0], p[1] - local[1], p[2] - local[2]];
    let mut bbox = e.bbox(t, b, pos, rot);
    let cxm = (bbox.max[0] + bbox.min[0]) / 2;
    let czm = (bbox.max[2] + bbox.min[2]) / 2;
    let world_min = g.t.world_min_y;
    let world_max = g.t.world_min_y + g.t.world_height - 1;
    let bottom = match cfg.project {
        None => pos[1],
        Some(ocean) => {
            if !column_has_biome(g, cxm, czm, world_min, world_max) {
                return None;
            }
            p[1] + ground_height(g, cxm, czm, ocean)
        }
    };
    let ground_delta = 1;
    let dy = bottom - (bbox.min[1] + ground_delta);
    bbox = bbox.moved([0, dy, 0]);
    let pos = [pos[0], pos[1] + dy, pos[2]];
    if (cfg.pad_bottom != 0 || cfg.pad_top != 0)
        && (bbox.min[1] < world_min + cfg.pad_bottom || bbox.max[1] > world_max - cfg.pad_top)
    {
        return None;
    }
    let cy = bottom + local[1];
    let start = [cxm, cy, czm];
    let mut pieces =
        vec![JigsawPiece { element: e, pos, ground_delta, rot, bbox, junctions: Vec::new(), waterlog: cfg.waterlog }];
    if cfg.size > 0 {
        let hmax = cfg.max_h;
        let vmax = cfg.max_v.unwrap_or(g.t.world_height);
        let free0 = Free::new(
            [cxm - hmax, (cy - vmax).max(world_min + cfg.pad_bottom), czm - hmax],
            [cxm + hmax + 1, (cy + vmax + 1).min(world_max + 1 - cfg.pad_top), czm + hmax + 1],
            vec![bbox],
        );
        assemble(g, cfg, &aliases, &mut pieces, free0, paths);
    }
    Some((start, pieces.into_iter().map(|p| Piece::Jigsaw(Box::new(p))).collect()))
}

/// A valid biome cell in the column (x, z) between two heights.
fn column_has_biome(g: &GenStart, x: i32, z: i32, y0: i32, y1: i32) -> bool {
    let mut qy = y0 >> 2;
    while qy <= y1 >> 2 {
        if g.biome_ok(x, qy << 2, z) {
            return true;
        }
        qy += 1;
    }
    false
}

/// Ground height of the column (first free y of the terrain, without decoration).
fn ground_height(g: &GenStart, x: i32, z: i32, ocean_floor: bool) -> i32 {
    #[cfg(test)]
    let started = std::time::Instant::now();
    let y = g.first_free(x, z, ocean_floor);
    #[cfg(test)]
    work::add(|w| {
        w.ground_reads += 1;
        w.ground_nanos += started.elapsed().as_nanos() as u64;
    });
    y
}

/// Pool designated by an identifier, after the aliases drawn for this start.
fn alias<'a>(aliases: &'a HashMap<String, String>, id: &'a str) -> &'a str {
    aliases.get(id).map_or(id, String::as_str)
}

fn attachable(j: &JunctionBlock, t: &JunctionBlock) -> bool {
    j.front == t.front.opposite() && (j.rollable || j.top == t.top) && (t.name.is_empty() || j.target == t.name)
}

/// Random draws of a trial known to fail: the shuffle of the four rotations, then, per rotation,
/// the shuffle of the element's `junctions` junctions (a rotation moves them, never changes their
/// number). A list of unit values has that length without any memory, and a shuffle draws the
/// same numbers whatever it moves, its rejection loop included.
fn replay_failed_trial(rng: &mut DecoRng, junctions: usize) {
    let mut rots = Rot::ALL;
    rng.shuffle(&mut rots);
    let mut units = vec![(); junctions];
    for _ in rots {
        rng.shuffle(&mut units);
    }
}

/// A piece waiting for its children: its free space and depth.
struct Pending {
    piece: usize,
    free: usize,
    depth: i32,
}

/// The piece whose junctions are being filled.
struct Source {
    index: usize,
    element: Arc<Element>,
    bbox: BBox,
    projection: Projection,
    ground_delta: i32,
    /// Free space of its children outside its own box.
    free: usize,
    depth: i32,
}

/// A junction of the source piece being filled, and what its trials share.
struct OpenJunction<'j> {
    jigsaw: &'j JunctionBlock,
    /// World position of the junction block, and of the block it faces.
    pos: [i32; 3],
    target: [i32; 3],
    /// Height of the junction block above the bottom of the source box.
    local_y: i32,
    /// Free space the child must fit in.
    free: usize,
    /// Ground height under the junction block, read at first need.
    ground: Option<i32>,
}

impl OpenJunction<'_> {
    fn ground(&mut self, g: &GenStart) -> i32 {
        *self.ground.get_or_insert_with(|| ground_height(g, self.pos[0], self.pos[2], false))
    }
}

/// A child that fits at the open junction.
struct Child {
    element: Arc<Element>,
    rot: Rot,
    pos: [i32; 3],
    bbox: BBox,
    projection: Projection,
    /// Height of its joining junction in its own box, bottom of that box, and height step from
    /// the source junction.
    local_y: i32,
    box_y: i32,
    dy: i32,
}

/// Outcome of the trial of a candidate element at a junction.
enum Trial {
    Placed,
    /// No rotation fits; each one shuffled `junctions` junctions.
    Failed {
        junctions: usize,
    },
}

/// State of the assembly of one start (§ 8.1).
struct Assembly<'c> {
    cfg: &'c JigsawCfg,
    aliases: &'c HashMap<String, String>,
    paths: Paths,
    pieces: &'c mut Vec<JigsawPiece>,
    frees: Vec<Free>,
    /// Free region private to each piece (created on demand).
    own_free: Vec<Option<usize>>,
    /// Pieces waiting for their children, by placement priority (highest first, then first in).
    queue: BTreeMap<i32, VecDeque<Pending>>,
    /// Expansion height per (element, rotation) for this start (`JIGSAW_EXPAND_MEMO`).
    expansion_heights: HashMap<(usize, Rot), i32>,
    /// Elements that failed at the junction being filled, and their junction count
    /// (`JIGSAW_FAILED_ELEMENT_MEMO`).
    failed_here: Vec<(*const Element, usize)>,
    /// Buffers kept from one trial to the next (`JIGSAW_REUSED_BUFFERS`).
    candidates: Vec<(bool, u32)>,
    target_order: Vec<u32>,
}

fn assemble(
    g: &mut GenStart,
    cfg: &JigsawCfg,
    aliases: &HashMap<String, String>,
    pieces: &mut Vec<JigsawPiece>,
    free0: Free,
    paths: Paths,
) {
    let mut a = Assembly {
        cfg,
        aliases,
        paths,
        pieces,
        frees: vec![free0],
        own_free: vec![None],
        queue: BTreeMap::new(),
        expansion_heights: HashMap::new(),
        failed_here: Vec::new(),
        candidates: Vec::new(),
        target_order: Vec::new(),
    };
    let mut current = Some(Pending { piece: 0, free: 0, depth: 0 });
    while let Some(p) = current.take() {
        a.grow(g, p);
        current = a.next_pending();
    }
}

impl Assembly<'_> {
    /// Next piece to grow: highest placement priority, first in.
    fn next_pending(&mut self) -> Option<Pending> {
        let mut e = self.queue.last_entry()?;
        let next = e.get_mut().pop_front();
        if e.get().is_empty() {
            e.remove();
        }
        next
    }

    /// Tries to attach a child to each junction of the piece, in its shuffled order.
    fn grow(&mut self, g: &mut GenStart, p: Pending) {
        let sg = g.sg;
        let (t, b) = (&*sg.templates, g.b);
        let piece = &self.pieces[p.piece];
        let (pos, rot) = (piece.pos, piece.rot);
        let src = Source {
            index: p.piece,
            element: piece.element.clone(),
            bbox: piece.bbox,
            projection: piece.element.projection(),
            ground_delta: piece.ground_delta,
            free: p.free,
            depth: p.depth,
        };
        let jigsaws = src.element.jigsaw_view(t, b, pos, rot, &mut g.rng, self.paths.precalc, self.paths.idx, None);
        if rc_data::env_flag!("RUSTCRAFT_JIGSAW_DEBUG") {
            eprintln!("piece {} ({:?}): {} junctions", p.piece, src.projection, jigsaws.len());
            for (j, _) in jigsaws.iter() {
                let found = sg.pools.get(alias(self.aliases, &j.pool)).is_some();
                eprintln!("   {:?} pool {} target {} pool found {found}", j.front, j.pool, j.target);
            }
        }
        for (j, jpos) in jigsaws.iter() {
            self.fill_junction(g, &src, j, jpos);
        }
    }

    /// Attaches the first candidate that fits to the junction `j` of the source piece.
    fn fill_junction(&mut self, g: &mut GenStart, src: &Source, j: &JunctionBlock, pos: [i32; 3]) {
        let sg = g.sg;
        let pool_id = alias(self.aliases, &j.pool);
        let Some(pool) = sg.pools.get(pool_id) else { return };
        if pool.templates.is_empty() && pool_id != "minecraft:empty" {
            return;
        }
        let Some(fallback) = sg.pools.get(&pool.fallback) else { return };
        if fallback.templates.is_empty() && pool.fallback != "minecraft:empty" {
            return;
        }
        let o = j.front.offset();
        let target = [pos[0] + o[0], pos[1] + o[1], pos[2] + o[2]];
        let free = if src.bbox.contains(target) { self.own_free_of(src) } else { src.free };
        let mut junction =
            OpenJunction { jigsaw: j, pos, target, local_y: pos[1] - src.bbox.min[1], free, ground: None };
        let mut copied: Vec<Arc<Element>> = Vec::new();
        let mut indexed = if self.paths.reused_buffers { std::mem::take(&mut self.candidates) } else { Vec::new() };
        self.shuffle_candidates(&mut g.rng, &pool, &fallback, src.depth, &mut copied, &mut indexed);
        let candidates = copied.iter().chain(
            indexed
                .iter()
                .map(|&(fb, i)| if fb { &fallback.templates[i as usize] } else { &pool.templates[i as usize] }),
        );
        self.failed_here.clear();
        for e in candidates {
            if matches!(**e, Element::Empty) {
                break;
            }
            let known = self.known_failure(e);
            #[cfg(test)]
            work::add(|w| {
                w.element_trials += 1;
                w.replayed_failures += u64::from(known.is_some());
            });
            if let Some(junctions) = known {
                replay_failed_trial(&mut g.rng, junctions);
                continue;
            }
            match self.try_element(g, src, &mut junction, e) {
                Trial::Placed => break,
                Trial::Failed { junctions } if self.paths.failed_element_memo => {
                    self.failed_here.push((Arc::as_ptr(e), junctions));
                }
                Trial::Failed { .. } => {}
            }
        }
        if self.paths.reused_buffers {
            self.candidates = indexed;
        }
    }

    /// Junction count of the element if it already failed at the junction being filled: a pool
    /// lists an element once per unit of weight, and a second trial meets the same free space.
    fn known_failure(&self, e: &Arc<Element>) -> Option<usize> {
        if !self.paths.failed_element_memo {
            return None;
        }
        self.failed_here.iter().find(|(tried, _)| std::ptr::eq(*tried, Arc::as_ptr(e))).map(|&(_, n)| n)
    }

    /// Free space private to the source piece (its own box), created at first need.
    fn own_free_of(&mut self, src: &Source) -> usize {
        if let Some(f) = self.own_free[src.index] {
            return f;
        }
        self.frees.push(Free::of_box(&src.bbox));
        self.own_free[src.index] = Some(self.frees.len() - 1);
        self.frees.len() - 1
    }

    /// Candidates of a junction in trial order: shuffled pool (except at maximum depth), then
    /// shuffled fallback. By indices (`idx`), the shuffle draws the same numbers (it depends only
    /// on the length) without copying the elements.
    fn shuffle_candidates(
        &self,
        rng: &mut DecoRng,
        pool: &Pool,
        fallback: &Pool,
        depth: i32,
        copied: &mut Vec<Arc<Element>>,
        indexed: &mut Vec<(bool, u32)>,
    ) {
        let with_pool = depth != self.cfg.size;
        indexed.clear();
        if !self.paths.idx {
            if with_pool {
                let mut v = pool.templates.clone();
                rng.shuffle(&mut v);
                copied.extend(v);
            }
            let mut v = fallback.templates.clone();
            rng.shuffle(&mut v);
            copied.extend(v);
        } else if self.paths.reused_buffers {
            // Each part shuffled in place in the kept buffer: same draws as a separate list.
            if with_pool {
                indexed.extend((0..pool.templates.len() as u32).map(|i| (false, i)));
                rng.shuffle(indexed);
            }
            let fallback_start = indexed.len();
            indexed.extend((0..fallback.templates.len() as u32).map(|i| (true, i)));
            rng.shuffle(&mut indexed[fallback_start..]);
        } else {
            if with_pool {
                let mut v: Vec<u32> = (0..pool.templates.len() as u32).collect();
                rng.shuffle(&mut v);
                indexed.extend(v.into_iter().map(|i| (false, i)));
            }
            let mut v: Vec<u32> = (0..fallback.templates.len() as u32).collect();
            rng.shuffle(&mut v);
            indexed.extend(v.into_iter().map(|i| (true, i)));
        }
    }

    /// Tries the element at the open junction over its four rotations, in shuffled order.
    fn try_element(&mut self, g: &mut GenStart, src: &Source, junction: &mut OpenJunction, e: &Arc<Element>) -> Trial {
        let sg = g.sg;
        let (t, b) = (&*sg.templates, g.b);
        let mut rots = Rot::ALL;
        g.rng.shuffle(&mut rots);
        let reuse = self.paths.reused_buffers;
        let mut order = if reuse { std::mem::take(&mut self.target_order) } else { Vec::new() };
        let (mut fit, mut junctions) = (None, 0);
        for r in rots {
            let view = e.jigsaw_view(
                t,
                b,
                [0, 0, 0],
                r,
                &mut g.rng,
                self.paths.precalc,
                self.paths.idx,
                reuse.then_some(&mut order),
            );
            junctions = view.len();
            fit = self.first_fit(g, src, junction, e, r, &view);
            if fit.is_some() {
                break;
            }
        }
        if reuse {
            self.target_order = order;
        }
        match fit {
            Some(child) => {
                self.place(g, src, junction, child);
                Trial::Placed
            }
            None => Trial::Failed { junctions },
        }
    }

    /// First junction of the element (rotation `r`) by which it joins the open junction with its
    /// box inside the free space.
    fn first_fit(
        &mut self,
        g: &GenStart,
        src: &Source,
        junction: &mut OpenJunction,
        e: &Arc<Element>,
        r: Rot,
        view: &JunctionView,
    ) -> Option<Child> {
        let (t, b) = (&*g.sg.templates, g.b);
        let hack_box = e.bbox(t, b, [0, 0, 0], r);
        let expand = self.expansion_height(g, e, r, view, &hack_box);
        let j = junction.jigsaw;
        let projection = e.projection();
        let both_rigid = src.projection == Projection::Rigid && projection == Projection::Rigid;
        let ys = src.bbox.min[1];
        for (x, xpos) in view.iter() {
            if !attachable(j, x) {
                continue;
            }
            let raw = [junction.target[0] - xpos[0], junction.target[1] - xpos[1], junction.target[2] - xpos[2]];
            // Box of the trial = box at zero position moved (same corners).
            let raw_bb = if self.paths.precalc { hack_box.moved(raw) } else { e.bbox(t, b, raw, r) };
            let local_y = xpos[1];
            let dy = junction.local_y - local_y + j.front.offset()[1];
            let box_y = if both_rigid { ys + dy } else { junction.ground(g) - local_y };
            let off = box_y - raw_bb.min[1];
            let mut bbox = raw_bb.moved([0, off, 0]);
            if expand > 0 {
                let n = (expand + 1).max(bbox.max[1] - bbox.min[1]);
                bbox.max[1] = bbox.max[1].max(bbox.min[1] + n);
            }
            if self.frees[junction.free].fits(&bbox, self.paths) {
                let pos = [raw[0], raw[1] + off, raw[2]];
                return Some(Child { element: e.clone(), rot: r, pos, bbox, projection, local_y, box_y, dy });
            }
        }
        None
    }

    /// Height to which the village trick stretches the trial boxes of the element (rotation `r`):
    /// the largest pool reachable through one of its junctions that faces into its own box.
    fn expansion_height(
        &mut self,
        g: &GenStart,
        e: &Arc<Element>,
        r: Rot,
        view: &JunctionView,
        hack_box: &BBox,
    ) -> i32 {
        if !self.cfg.expansion_hack || hack_box.span(1) > 16 {
            return 0;
        }
        if !self.paths.expand_memo {
            return self.expansion_height_of(g, view, hack_box);
        }
        // Same element, same rotation: same junctions at zero position and same pools for the
        // whole start (its aliases are drawn once), hence the same height.
        let key = (Arc::as_ptr(e) as usize, r);
        if let Some(&h) = self.expansion_heights.get(&key) {
            return h;
        }
        let h = self.expansion_height_of(g, view, hack_box);
        self.expansion_heights.insert(key, h);
        h
    }

    fn expansion_height_of(&self, g: &GenStart, view: &JunctionView, hack_box: &BBox) -> i32 {
        let (pools, t, b) = (&g.sg.pools, &*g.sg.templates, g.b);
        view.iter()
            .map(|(x, xpos)| {
                let o = x.front.offset();
                if !hack_box.contains([xpos[0] + o[0], xpos[1] + o[1], xpos[2] + o[2]]) {
                    return 0;
                }
                let child = pools.get(alias(self.aliases, &x.pool));
                let a = child.as_ref().map_or(0, |p| p.max_size(t, b));
                let fb = child.and_then(|p| pools.get(&p.fallback)).map_or(0, |p| p.max_size(t, b));
                a.max(fb)
            })
            .max()
            .unwrap_or(0)
    }

    /// Places the child: its box leaves the free space, both pieces record the junction, and the
    /// child waits for its own children (below maximum depth).
    fn place(&mut self, g: &GenStart, src: &Source, junction: &mut OpenJunction, c: Child) {
        let child_rigid = c.projection == Projection::Rigid;
        self.frees[junction.free].remove(c.bbox, self.paths);
        let ground_delta = if child_rigid { src.ground_delta - c.dy } else { 1 };
        let junction_y = if src.projection == Projection::Rigid {
            src.bbox.min[1] + junction.local_y
        } else if child_rigid {
            c.box_y + c.local_y
        } else {
            junction.ground(g) + c.dy / 2
        };
        self.pieces[src.index].junctions.push(Junction {
            x: junction.target[0],
            ground_y: junction_y - junction.local_y + src.ground_delta,
            z: junction.target[2],
            dy: c.dy,
            projection: c.projection,
        });
        self.pieces.push(JigsawPiece {
            element: c.element,
            pos: c.pos,
            ground_delta,
            rot: c.rot,
            bbox: c.bbox,
            junctions: vec![Junction {
                x: junction.pos[0],
                ground_y: junction_y - c.local_y + ground_delta,
                z: junction.pos[2],
                dy: -c.dy,
                projection: src.projection,
            }],
            waterlog: self.cfg.waterlog,
        });
        self.own_free.push(None);
        if src.depth < self.cfg.size {
            let child = Pending { piece: self.pieces.len() - 1, free: junction.free, depth: src.depth + 1 };
            self.queue.entry(junction.jigsaw.placement_priority).or_default().push_back(child);
        }
    }
}
