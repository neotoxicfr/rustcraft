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
        pre: OnceLock<Option<Box<SinglePre>>>,
        /// Processors analysed once and shared by the placements (`RUSTCRAFT_OPT_PROCS_SHARED`).
        procs: OnceLock<Arc<Vec<crate::structure_place::Proc>>>,
    },
    List { elements: Vec<Arc<Element>>, projection: Projection },
    Feature { feature: String, projection: Projection },
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
pub struct WJigsaw {
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

impl WJigsaw {
    /// Junction of the rotated template, at zero position.
    fn turned(j: &crate::template::Jigsaw, rot: Rot) -> WJigsaw {
        WJigsaw {
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
static FEATURE_JIGSAW: LazyLock<[WJigsaw; 1]> = LazyLock::new(|| {
    [WJigsaw {
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
pub struct SinglePre {
    bbox0: [BBox; 4],
    jigs: [Vec<Vec<WJigsaw>>; 4],
}

impl SinglePre {
    fn new(tp: &Template) -> SinglePre {
        SinglePre {
            bbox0: Rot::ALL.map(|r| tp.bbox([0, 0, 0], &Settings::rot(r))),
            jigs: Rot::ALL.map(|r| tp.jigsaws.iter().map(|pal| pal.iter().map(|j| WJigsaw::turned(j, r)).collect()).collect()),
        }
    }
}

/// Junctions of a piece in trial order: base list, offset, order (`None` = list).
pub(crate) struct JView<'a> {
    list: Cow<'a, [WJigsaw]>,
    off: [i32; 3],
    order: Option<Vec<u32>>,
}

impl JView<'_> {
    /// (junction, world position).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&WJigsaw, [i32; 3])> {
        let n = self.order.as_ref().map_or(self.list.len(), Vec::len);
        (0..n).map(move |k| {
            let j = &self.list[self.order.as_ref().map_or(k, |o| o[k] as usize)];
            (j, [j.pos[0] + self.off[0], j.pos[1] + self.off[1], j.pos[2] + self.off[2]])
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.order.as_ref().map_or(self.list.len(), Vec::len)
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

    fn pre(&self, t: &Templates, b: &Blocks) -> Option<&SinglePre> {
        match self {
            Element::Single { pre, .. } => {
                pre.get_or_init(|| self.template(t, b).map(|tp| Box::new(SinglePre::new(&tp)))).as_deref()
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
    ) -> Vec<WJigsaw> {
        match self {
            Element::Single { .. } => {
                let Some(tp) = self.template(t, b) else { return Vec::new() };
                if tp.palettes.is_empty() {
                    return Vec::new();
                }
                let pi = palette_at(pos, tp.palettes.len());
                let mut v: Vec<WJigsaw> = tp.jigsaws[pi]
                    .iter()
                    .map(|j| {
                        let w = WJigsaw::turned(j, rot);
                        WJigsaw { pos: [w.pos[0] + pos[0], w.pos[1] + pos[1], w.pos[2] + pos[2]], ..w }
                    })
                    .collect();
                rng.shuffle(&mut v);
                v.sort_by_key(|j| std::cmp::Reverse(j.selection_priority));
                v
            }
            Element::List { elements, .. } => {
                elements.first().map(|e| e.shuffled_jigsaws(t, b, pos, rot, rng)).unwrap_or_default()
            }
            Element::Feature { .. } => vec![WJigsaw { pos, ..FEATURE_JIGSAW[0].clone() }],
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
    ) -> (Cow<'a, [WJigsaw]>, [i32; 3], bool) {
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
                    (Cow::Owned(tp.jigsaws[pi].iter().map(|j| WJigsaw::turned(j, rot)).collect()), pos, true)
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
    /// lists (`precalc`), order by indices without copy (`idx`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn jigsaw_view<'a>(
        &'a self,
        t: &Templates,
        b: &Blocks,
        pos: [i32; 3],
        rot: Rot,
        rng: &mut DecoRng,
        precalc: bool,
        idx: bool,
    ) -> JView<'a> {
        if !precalc && !idx {
            return JView { list: Cow::Owned(self.shuffled_jigsaws(t, b, pos, rot, rng)), off: [0; 3], order: None };
        }
        let (list, off, mix) = self.base_list(t, b, pos, rot, precalc);
        if idx {
            let mut order: Vec<u32> = (0..list.len() as u32).collect();
            if mix {
                // The shuffle depends only on the length: same draws, same permutation.
                rng.shuffle(&mut order);
                order.sort_by_key(|&i| std::cmp::Reverse(list[i as usize].selection_priority));
            }
            return JView { list, off, order: Some(order) };
        }
        let mut v: Vec<WJigsaw> = list
            .iter()
            .map(|j| WJigsaw { pos: [j.pos[0] + off[0], j.pos[1] + off[1], j.pos[2] + off[2]], ..j.clone() })
            .collect();
        if mix {
            rng.shuffle(&mut v);
            v.sort_by_key(|j| std::cmp::Reverse(j.selection_priority));
        }
        JView { list: Cow::Owned(v), off: [0; 3], order: None }
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
    /// Frozen table: text of the pool and pool analysed at first need (`RUSTCRAFT_OPT_POOLS_TABLE`).
    raw: HashMap<String, (Json, OnceLock<Arc<Pool>>)>,
    /// Old path: cache under lock, absent identifiers included.
    cache: Mutex<HashMap<String, Option<Arc<Pool>>>>,
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
        let mut raw = HashMap::new();
        for id in d.ids("template_pool") {
            if let Ok(j) = d.json("template_pool", &id) {
                raw.insert(id, (j, OnceLock::new()));
            }
        }
        Pools { raw, cache: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, id: &str) -> Option<Arc<Pool>> {
        if crate::opt::pools_table() {
            let (j, cell) = self.raw.get(id)?;
            return Some(cell.get_or_init(|| parse_pool(j)).clone());
        }
        if let Some(p) = self.cache.lock().ok()?.get(id) {
            return p.clone();
        }
        let p = self.raw.get(id).map(|(j, _)| parse_pool(j));
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

/// Free volume: real cuboid minus removed boxes.
#[derive(Debug, Clone)]
struct Free {
    lo: [i32; 3],
    /// Exclusive upper bounds.
    hi: [i32; 3],
    removed: Vec<BBox>,
}

impl Free {
    fn of_box(b: &BBox) -> Free {
        Free { lo: b.min, hi: [b.max[0] + 1, b.max[1] + 1, b.max[2] + 1], removed: Vec::new() }
    }

    /// The box shrunk by 0.25 per face is entirely free.
    fn fits(&self, b: &BBox) -> bool {
        (0..3).all(|i| b.min[i] >= self.lo[i] && b.max[i] < self.hi[i]) && !self.removed.iter().any(|r| r.intersects(b))
    }
}

struct Pending {
    piece: usize,
    free: usize,
    depth: i32,
}

/// Start of a jigsaw structure (§ 8.1).
pub fn generate(g: &mut GenStart, cfg: &JigsawCfg) -> Option<([i32; 3], Vec<Piece>)> {
    let sg = g.sg;
    let (t, b) = (&*sg.templates, g.b);
    let h = cfg.start_height.sample(&mut g.rng, g.hctx);
    let p = [16 * g.cx, h, 16 * g.cz];
    let aliases = resolve_aliases(cfg, sg.seed, p);
    let (precalc, idx) = (crate::opt::jigsaw_precalc(), crate::opt::jigsaw_idx());
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
            let v = e.jigsaw_view(t, b, p, rot, &mut g.rng, precalc, idx);
            v.iter().find(|(j, _)| *j.name == **name)?.1
        }
        None => p,
    };
    let local = [anchor[0] - p[0], anchor[1] - p[1], anchor[2] - p[2]];
    let pos = [p[0] - local[0], p[1] - local[1], p[2] - local[2]];
    let mut bbox = e.bbox(t, b, pos, rot);
    let cxm = (bbox.max[0] + bbox.min[0]) / 2;
    let czm = (bbox.max[2] + bbox.min[2]) / 2;
    let min_y = g.hctx.min_y.min(g.t.world_min_y);
    let world_min = g.t.world_min_y;
    let world_max = g.t.world_min_y + g.t.world_height - 1;
    let _ = min_y;
    let bottom = match cfg.project {
        None => pos[1],
        Some(ocean) => {
            if !column_has_biome(g, cxm, czm, world_min, world_max) {
                return None;
            }
            p[1] + g.first_free(cxm, czm, ocean)
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
        let free0 = Free {
            lo: [cxm - hmax, (cy - vmax).max(world_min + cfg.pad_bottom), czm - hmax],
            hi: [cxm + hmax + 1, (cy + vmax + 1).min(world_max + 1 - cfg.pad_top), czm + hmax + 1],
            removed: vec![bbox],
        };
        assemble(g, cfg, &aliases, &mut pieces, free0, precalc, idx);
    }
    Some((start, pieces.into_iter().map(|p| Piece::Jigsaw(Box::new(p))).collect()))
}

/// A valid biome cell in the column (x, z) between two heights.
fn column_has_biome(g: &GenStart, x: i32, z: i32, y0: i32, y1: i32) -> bool {
    let mut qy = y0 >> 2;
    while qy <= y1 >> 2 {
        if g.def.biomes.get(g.biome_at(x, qy << 2, z) as usize).copied().unwrap_or(false) {
            return true;
        }
        qy += 1;
    }
    false
}

/// Pool designated by an identifier, after the aliases drawn for this start.
fn alias<'a>(aliases: &'a HashMap<String, String>, id: &'a str) -> &'a str {
    aliases.get(id).map_or(id, String::as_str)
}

fn attachable(j: &WJigsaw, t: &WJigsaw) -> bool {
    j.front == t.front.opposite() && (j.rollable || j.top == t.top) && (t.name.is_empty() || j.target == t.name)
}

fn assemble(
    g: &mut GenStart,
    cfg: &JigsawCfg,
    aliases: &HashMap<String, String>,
    pieces: &mut Vec<JigsawPiece>,
    free0: Free,
    precalc: bool,
    idx: bool,
) {
    let sg = g.sg;
    let (t, b) = (&*sg.templates, g.b);
    let mut frees: Vec<Free> = vec![free0];
    // Free region private to each piece (created on demand).
    let mut own_free: Vec<Option<usize>> = vec![None];
    let mut queue: BTreeMap<i32, VecDeque<Pending>> = BTreeMap::new();
    let mut current = Some(Pending { piece: 0, free: 0, depth: 0 });
    while let Some(Pending { piece: si, free: ctx_free, depth }) = current.take() {
        let (src_e, src_pos, src_rot, src_bbox, src_ground) = {
            let s = &pieces[si];
            (s.element.clone(), s.pos, s.rot, s.bbox, s.ground_delta)
        };
        let src_proj = src_e.projection();
        let src_rigid = src_proj == Projection::Rigid;
        let ys = src_bbox.min[1];
        let jigsaws = src_e.jigsaw_view(t, b, src_pos, src_rot, &mut g.rng, precalc, idx);
        if rc_data::env_flag!("RUSTCRAFT_JIGSAW_DEBUG") {
            eprintln!("piece {si} ({src_proj:?}): {} junctions", jigsaws.len());
            for (j, _) in jigsaws.iter() {
                eprintln!(
                    "   {:?} pool {} target {} pool found {}",
                    j.front,
                    j.pool,
                    j.target,
                    sg.pools.get(alias(aliases, &j.pool)).is_some()
                );
            }
        }
        'outer: for (j, jpos) in jigsaws.iter() {
            let dir = j.front;
            let o = dir.offset();
            let target = [jpos[0] + o[0], jpos[1] + o[1], jpos[2] + o[2]];
            let yj = jpos[1] - ys;
            let mut base = i32::MIN;
            let pool_id = alias(aliases, &j.pool);
            let Some(pool) = sg.pools.get(pool_id) else { continue };
            if pool.templates.is_empty() && pool_id != "minecraft:empty" {
                continue;
            }
            let Some(fallback) = sg.pools.get(&pool.fallback) else { continue };
            if fallback.templates.is_empty() && pool.fallback != "minecraft:empty" {
                continue;
            }
            let inside = src_bbox.contains(target);
            let free_i = if inside {
                match own_free[si] {
                    Some(f) => f,
                    None => {
                        frees.push(Free::of_box(&src_bbox));
                        own_free[si] = Some(frees.len() - 1);
                        frees.len() - 1
                    }
                }
            } else {
                ctx_free
            };
            // Candidates: shuffled pool (except at maximum depth) then shuffled fallback. By
            // indices, the shuffle draws the same numbers (it depends only on the length).
            let mut cands_arc: Vec<Arc<Element>> = Vec::new();
            let mut cands_idx: Vec<(bool, u32)> = Vec::new();
            if idx {
                if depth != cfg.size {
                    let mut v: Vec<u32> = (0..pool.templates.len() as u32).collect();
                    g.rng.shuffle(&mut v);
                    cands_idx.extend(v.into_iter().map(|i| (false, i)));
                }
                let mut v: Vec<u32> = (0..fallback.templates.len() as u32).collect();
                g.rng.shuffle(&mut v);
                cands_idx.extend(v.into_iter().map(|i| (true, i)));
            } else {
                if depth != cfg.size {
                    let mut v = pool.templates.clone();
                    g.rng.shuffle(&mut v);
                    cands_arc.extend(v);
                }
                let mut v = fallback.templates.clone();
                g.rng.shuffle(&mut v);
                cands_arc.extend(v);
            }
            let cands = cands_arc.iter().chain(
                cands_idx
                    .iter()
                    .map(|&(fb, i)| if fb { &fallback.templates[i as usize] } else { &pool.templates[i as usize] }),
            );
            for e in cands {
                if matches!(**e, Element::Empty) {
                    break;
                }
                let tgt_proj = e.projection();
                let tgt_rigid = tgt_proj == Projection::Rigid;
                let mut rots = Rot::ALL;
                g.rng.shuffle(&mut rots);
                for r in rots {
                    let tj = e.jigsaw_view(t, b, [0, 0, 0], r, &mut g.rng, precalc, idx);
                    let hack_box = e.bbox(t, b, [0, 0, 0], r);
                    let expand = if cfg.expansion_hack && hack_box.span(1) <= 16 {
                        tj.iter()
                            .map(|(x, xpos)| {
                                let o = x.front.offset();
                                if !hack_box.contains([xpos[0] + o[0], xpos[1] + o[1], xpos[2] + o[2]]) {
                                    return 0;
                                }
                                let cp = sg.pools.get(alias(aliases, &x.pool));
                                let a = cp.as_ref().map_or(0, |p| p.max_size(t, b));
                                let fb = cp.and_then(|p| sg.pools.get(&p.fallback)).map_or(0, |p| p.max_size(t, b));
                                a.max(fb)
                            })
                            .max()
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    for (x, xpos) in tj.iter() {
                        if !attachable(j, x) {
                            continue;
                        }
                        let raw = [target[0] - xpos[0], target[1] - xpos[1], target[2] - xpos[2]];
                        // Box of the trial = box at zero position moved (same corners).
                        let raw_bb = if precalc { hack_box.moved(raw) } else { e.bbox(t, b, raw, r) };
                        let yt = xpos[1];
                        let dy = yj - yt + dir.offset()[1];
                        let yb = if src_rigid && tgt_rigid {
                            ys + dy
                        } else {
                            if base == i32::MIN {
                                base = g.first_free(jpos[0], jpos[2], false);
                            }
                            base - yt
                        };
                        let off = yb - raw_bb.min[1];
                        let mut bt = raw_bb.moved([0, off, 0]);
                        let post = [raw[0], raw[1] + off, raw[2]];
                        if expand > 0 {
                            let n = (expand + 1).max(bt.max[1] - bt.min[1]);
                            bt.max[1] = bt.max[1].max(bt.min[1] + n);
                        }
                        if !frees[free_i].fits(&bt) {
                            continue;
                        }
                        frees[free_i].removed.push(bt);
                        let sol_t = if tgt_rigid { src_ground - dy } else { 1 };
                        let jy = if src_rigid {
                            ys + yj
                        } else if tgt_rigid {
                            yb + yt
                        } else {
                            if base == i32::MIN {
                                base = g.first_free(jpos[0], jpos[2], false);
                            }
                            base + dy / 2
                        };
                        pieces[si].junctions.push(Junction {
                            x: target[0],
                            ground_y: jy - yj + src_ground,
                            z: target[2],
                            dy,
                            projection: tgt_proj,
                        });
                        pieces.push(JigsawPiece {
                            element: e.clone(),
                            pos: post,
                            ground_delta: sol_t,
                            rot: r,
                            bbox: bt,
                            junctions: vec![Junction {
                                x: jpos[0],
                                ground_y: jy - yt + sol_t,
                                z: jpos[2],
                                dy: -dy,
                                projection: src_proj,
                            }],
                            waterlog: cfg.waterlog,
                        });
                        own_free.push(None);
                        if depth < cfg.size {
                            queue.entry(j.placement_priority).or_default().push_back(Pending {
                                piece: pieces.len() - 1,
                                free: free_i,
                                depth: depth + 1,
                            });
                        }
                        continue 'outer;
                    }
                }
            }
        }
        // Next: highest priority, first in.
        if let Some(mut e) = queue.last_entry() {
            current = e.get_mut().pop_front();
            if e.get().is_empty() {
                e.remove();
            }
        }
    }
}
