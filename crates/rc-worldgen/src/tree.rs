//! Trees and fallen trunks (`generation-decoration.md` § 5): trunk, foliage, root
//! placers, decorators, recomputation of the leaf distances, Java-style hash sets.

use crate::Error;
use crate::decor::Ctx;
use crate::decor_blocks::{Blocks, Dir, State};
use crate::decor_pred::{Blockset, IntP, ParseCtx, Provider, get_f32, get_i32, opt_bool, opt_f32, opt_i32};
use crate::decor_region::{BeData, Hm, Region, add, add_n};
use crate::decor_rng::DecoRng;
use crate::feature::Registry;
use crate::json::Json;

pub use crate::jset::JSet;

/// Buffers reused from one tree to the next.
#[derive(Debug, Default)]
pub struct Scratch {
    pub roots: JSet,
    pub trunks: JSet,
    pub foliage: JSet,
    pub decorations: JSet,
    pub logs: Vec<[i32; 3]>,
    pub leaves: Vec<[i32; 3]>,
    pub root_list: Vec<[i32; 3]>,
    pub tmp: Vec<[i32; 3]>,
    pub to_check: [JSet; 7],
    pub shape: Vec<u64>,
    pub ore_data: Vec<f64>,
    pub ore_tested: Vec<u64>,
    pub attach: Vec<Attachment>,
    /// Reused list copies (`RUSTCRAFT_OPT_TREE_TAKE`).
    pool: Vec<Vec<[i32; 3]>>,
    black: hashbrown::HashSet<[i32; 3]>,
}

/// Decorator context list copied into a buffer taken back from the reserve
/// (`RUSTCRAFT_OPT_TREE_TAKE`, default) or into a new vector (`=0`).
fn copy_list(s: &mut Scratch, which: fn(&Scratch) -> &Vec<[i32; 3]>) -> Vec<[i32; 3]> {
    if crate::opt::tree_take() {
        let mut v = s.pool.pop().unwrap_or_default();
        v.clear();
        v.extend_from_slice(which(s));
        v
    } else {
        which(s).clone()
    }
}

/// Hands a copy back to the reserve.
fn recycle(s: &mut Scratch, v: Vec<[i32; 3]>) {
    if crate::opt::tree_take() {
        s.pool.push(v);
    }
}

fn logs_of(s: &Scratch) -> &Vec<[i32; 3]> {
    &s.logs
}

fn leaves_of(s: &Scratch) -> &Vec<[i32; 3]> {
    &s.leaves
}

// ---------------------------------------------------------------- Configuration

#[derive(Debug, Clone)]
pub enum TrunkKind {
    Straight,
    Forking,
    Giant,
    MegaJungle,
    DarkOak,
    Fancy,
    Bending { min_leaves: i32, bend: IntP },
    Upwards { steps: IntP, prob: f32, len: IntP, through: Blockset },
    Cherry { count: IntP, hlen: IntP, start: (i32, i32), end: IntP },
    Poplar { above: IntP, amount: IntP },
}

#[derive(Debug, Clone)]
pub struct Trunk {
    pub kind: TrunkKind,
    pub base: i32,
    pub a: i32,
    pub b: i32,
}

#[derive(Debug, Clone)]
pub enum FoliageKind {
    Blob(i32),
    Bush(i32),
    Fancy(i32),
    Spruce(IntP),
    Pine(IntP),
    MegaPine(IntP),
    Jungle(i32),
    Acacia,
    DarkOak,
    Cherry { height: IntP, wide_hole: f32, corner_hole: f32, hanging: f32, hanging_ext: f32 },
    RandomSpread { height: IntP, attempts: i32 },
    Poplar { height: IntP, side_hole: f32 },
}

#[derive(Debug, Clone)]
pub struct Foliage {
    pub kind: FoliageKind,
    pub radius: IntP,
    pub offset: IntP,
}

#[derive(Debug, Clone)]
pub struct MangroveRoots {
    pub trunk_offset_y: IntP,
    pub provider: Provider,
    pub above: Option<(Provider, f32)>,
    pub can_grow_through: Blockset,
    pub muddy_in: Blockset,
    pub muddy_provider: Provider,
    pub max_width: i32,
    pub max_length: i32,
    pub skew: f32,
}

#[derive(Debug, Clone)]
pub enum TreeDecorator {
    TrunkVine,
    LeaveVine(f32),
    Cocoa(f32),
    Beehive(f32),
    AlterGround(Provider),
    AttachedToLeaves { prob: f32, ex_xz: i32, ex_y: i32, provider: Provider, required: i32, dirs: Vec<Dir> },
    AttachedToLogs { prob: f32, provider: Provider, dirs: Vec<Dir> },
    PlaceOnGround { tries: i32, radius: i32, height: i32, provider: Provider },
    PaleMoss { leaves: f32, trunk: f32, ground: f32 },
    CreakingHeart(f32),
    ShelfMushroom(f32),
}

#[derive(Debug, Clone)]
pub struct Tree {
    pub trunk_provider: Provider,
    pub trunk: Trunk,
    pub foliage_provider: Provider,
    pub foliage: Foliage,
    pub roots: Option<MangroveRoots>,
    /// (limit, bottom, middle, top, upper limit, three-layer)
    pub size: (i32, i32, i32, i32, i32, bool),
    pub min_clipped: Option<i32>,
    pub decorators: Vec<TreeDecorator>,
    pub ignore_vines: bool,
    pub below_trunk: Provider,
}

#[derive(Debug, Clone)]
pub struct FallenTree {
    pub trunk_provider: Provider,
    pub log_length: IntP,
    pub stump_decorators: Vec<TreeDecorator>,
    pub log_decorators: Vec<TreeDecorator>,
}

fn bad(w: impl Into<String>) -> Error {
    Error::Format(w.into())
}

fn ty(j: &Json) -> Result<&str, Error> {
    let t = j.get("type").and_then(Json::as_str).ok_or_else(|| bad("without type"))?;
    Ok(t.strip_prefix("minecraft:").unwrap_or(t))
}

fn int(j: &Json, k: &str) -> Result<IntP, Error> {
    IntP::parse(j.get(k).ok_or_else(|| bad(k.to_owned()))?)
}

fn provider(j: &Json, k: &str, c: &mut ParseCtx) -> Result<Provider, Error> {
    Provider::parse(j.get(k).ok_or_else(|| bad(k.to_owned()))?, c)
}

fn dirs(j: &Json) -> Result<Vec<Dir>, Error> {
    j.as_array()
        .ok_or_else(|| bad("directions"))?
        .iter()
        .map(|d| d.as_str().and_then(Dir::from_name).ok_or_else(|| bad("direction")))
        .collect()
}

fn parse_decorator(j: &Json, c: &mut ParseCtx) -> Result<TreeDecorator, Error> {
    Ok(match ty(j)? {
        "trunk_vine" => TreeDecorator::TrunkVine,
        "leave_vine" => TreeDecorator::LeaveVine(get_f32(j, "probability")?),
        "cocoa" => TreeDecorator::Cocoa(get_f32(j, "probability")?),
        "beehive" => TreeDecorator::Beehive(get_f32(j, "probability")?),
        "alter_ground" => TreeDecorator::AlterGround(provider(j, "provider", c)?),
        "attached_to_leaves" => TreeDecorator::AttachedToLeaves {
            prob: get_f32(j, "probability")?,
            ex_xz: get_i32(j, "exclusion_radius_xz")?,
            ex_y: get_i32(j, "exclusion_radius_y")?,
            provider: provider(j, "block_provider", c)?,
            required: get_i32(j, "required_empty_blocks")?,
            dirs: dirs(j.get("directions").ok_or_else(|| bad("directions"))?)?,
        },
        "attached_to_logs" => TreeDecorator::AttachedToLogs {
            prob: get_f32(j, "probability")?,
            provider: provider(j, "block_provider", c)?,
            dirs: dirs(j.get("directions").ok_or_else(|| bad("directions"))?)?,
        },
        "place_on_ground" => TreeDecorator::PlaceOnGround {
            tries: opt_i32(j, "tries", 128),
            radius: opt_i32(j, "radius", 2),
            height: opt_i32(j, "height", 1),
            provider: provider(j, "block_state_provider", c)?,
        },
        "pale_moss" => TreeDecorator::PaleMoss {
            leaves: get_f32(j, "leaves_probability")?,
            trunk: get_f32(j, "trunk_probability")?,
            ground: get_f32(j, "ground_probability")?,
        },
        "creaking_heart" => TreeDecorator::CreakingHeart(get_f32(j, "probability")?),
        "shelf_mushroom" => TreeDecorator::ShelfMushroom(get_f32(j, "probability")?),
        t => return Err(bad(format!("decorator {t}"))),
    })
}

pub fn parse_tree(j: &Json, c: &mut ParseCtx, _reg: &mut Registry) -> Result<Tree, Error> {
    let tp = j.get("trunk_placer").ok_or_else(|| bad("trunk_placer"))?;
    let kind = match ty(tp)? {
        "straight_trunk_placer" => TrunkKind::Straight,
        "forking_trunk_placer" => TrunkKind::Forking,
        "giant_trunk_placer" => TrunkKind::Giant,
        "mega_jungle_trunk_placer" => TrunkKind::MegaJungle,
        "dark_oak_trunk_placer" => TrunkKind::DarkOak,
        "fancy_trunk_placer" => TrunkKind::Fancy,
        "bending_trunk_placer" => {
            TrunkKind::Bending { min_leaves: opt_i32(tp, "min_height_for_leaves", 1), bend: int(tp, "bend_length")? }
        }
        "upwards_branching_trunk_placer" => TrunkKind::Upwards {
            steps: int(tp, "extra_branch_steps")?,
            prob: get_f32(tp, "place_branch_per_log_probability")?,
            len: int(tp, "extra_branch_length")?,
            through: Blockset::parse(tp.get("can_grow_through").ok_or_else(|| bad("can_grow_through"))?, c.b)?,
        },
        "cherry_trunk_placer" => {
            let s = tp.get("branch_start_offset_from_top").ok_or_else(|| bad("branch_start_offset_from_top"))?;
            TrunkKind::Cherry {
                count: int(tp, "branch_count")?,
                hlen: int(tp, "branch_horizontal_length")?,
                start: (get_i32(s, "min_inclusive")?, get_i32(s, "max_inclusive")?),
                end: int(tp, "branch_end_offset_from_top")?,
            }
        }
        "poplar_trunk_placer" => {
            TrunkKind::Poplar { above: int(tp, "trunk_height_above_branches")?, amount: int(tp, "branch_amount")? }
        }
        t => return Err(bad(format!("trunk placer {t}"))),
    };
    let trunk = Trunk {
        kind,
        base: get_i32(tp, "base_height")?,
        a: get_i32(tp, "height_rand_a")?,
        b: get_i32(tp, "height_rand_b")?,
    };
    let fp = j.get("foliage_placer").ok_or_else(|| bad("foliage_placer"))?;
    let fkind = match ty(fp)? {
        "blob_foliage_placer" => FoliageKind::Blob(get_i32(fp, "height")?),
        "bush_foliage_placer" => FoliageKind::Bush(get_i32(fp, "height")?),
        "fancy_foliage_placer" => FoliageKind::Fancy(get_i32(fp, "height")?),
        "spruce_foliage_placer" => FoliageKind::Spruce(int(fp, "trunk_height")?),
        "pine_foliage_placer" => FoliageKind::Pine(int(fp, "height")?),
        "mega_pine_foliage_placer" => FoliageKind::MegaPine(int(fp, "crown_height")?),
        "jungle_foliage_placer" => FoliageKind::Jungle(get_i32(fp, "height")?),
        "acacia_foliage_placer" => FoliageKind::Acacia,
        "dark_oak_foliage_placer" => FoliageKind::DarkOak,
        "cherry_foliage_placer" => FoliageKind::Cherry {
            height: int(fp, "height")?,
            wide_hole: get_f32(fp, "wide_bottom_layer_hole_chance")?,
            corner_hole: get_f32(fp, "corner_hole_chance")?,
            hanging: get_f32(fp, "hanging_leaves_chance")?,
            hanging_ext: get_f32(fp, "hanging_leaves_extension_chance")?,
        },
        "random_spread_foliage_placer" => FoliageKind::RandomSpread {
            height: int(fp, "foliage_height")?,
            attempts: get_i32(fp, "leaf_placement_attempts")?,
        },
        "poplar_foliage_placer" => {
            FoliageKind::Poplar { height: int(fp, "height")?, side_hole: get_f32(fp, "side_hole_chance")? }
        }
        t => return Err(bad(format!("foliage placer {t}"))),
    };
    let foliage = Foliage { kind: fkind, radius: int(fp, "radius")?, offset: int(fp, "offset")? };
    let roots = match j.get("root_placer") {
        None => None,
        Some(rp) => {
            let m = rp.get("mangrove_root_placement").ok_or_else(|| bad("mangrove_root_placement"))?;
            let above = match rp.get("above_root_placement") {
                Some(a) => Some((provider(a, "above_root_provider", c)?, get_f32(a, "above_root_placement_chance")?)),
                None => None,
            };
            Some(MangroveRoots {
                trunk_offset_y: int(rp, "trunk_offset_y")?,
                provider: provider(rp, "root_provider", c)?,
                above,
                can_grow_through: Blockset::parse(
                    m.get("can_grow_through").ok_or_else(|| bad("can_grow_through"))?,
                    c.b,
                )?,
                muddy_in: Blockset::parse(m.get("muddy_roots_in").ok_or_else(|| bad("muddy_roots_in"))?, c.b)?,
                muddy_provider: provider(m, "muddy_roots_provider", c)?,
                max_width: get_i32(m, "max_root_width")?,
                max_length: get_i32(m, "max_root_length")?,
                skew: get_f32(m, "random_skew_chance")?,
            })
        }
    };
    let ms = j.get("minimum_size").ok_or_else(|| bad("minimum_size"))?;
    let three = ty(ms)? == "three_layers_feature_size";
    let size = if three {
        (
            opt_i32(ms, "limit", 1),
            opt_i32(ms, "lower_size", 0),
            opt_i32(ms, "middle_size", 1),
            opt_i32(ms, "upper_size", 1),
            opt_i32(ms, "upper_limit", 1),
            true,
        )
    } else {
        (opt_i32(ms, "limit", 1), opt_i32(ms, "lower_size", 0), 0, opt_i32(ms, "upper_size", 1), 0, false)
    };
    let mut decorators = Vec::new();
    for d in j.get("decorators").and_then(Json::as_array).unwrap_or(&[]) {
        decorators.push(parse_decorator(d, c)?);
    }
    Ok(Tree {
        trunk_provider: provider(j, "trunk_provider", c)?,
        trunk,
        foliage_provider: provider(j, "foliage_provider", c)?,
        foliage,
        roots,
        size,
        min_clipped: ms.get("min_clipped_height").and_then(Json::as_i32),
        decorators,
        ignore_vines: opt_bool(j, "ignore_vines", false),
        below_trunk: provider(j, "below_trunk_provider", c)?,
    })
}

pub fn parse_fallen(j: &Json, c: &mut ParseCtx) -> Result<FallenTree, Error> {
    let list = |k: &str, c: &mut ParseCtx| -> Result<Vec<TreeDecorator>, Error> {
        j.get(k).and_then(Json::as_array).unwrap_or(&[]).iter().map(|d| parse_decorator(d, c)).collect()
    };
    Ok(FallenTree {
        trunk_provider: provider(j, "trunk_provider", c)?,
        log_length: int(j, "log_length")?,
        stump_decorators: list("stump_decorators", c)?,
        log_decorators: list("log_decorators", c)?,
    })
}

// ---------------------------------------------------------------- Placement

#[derive(Debug, Clone, Copy)]
pub struct Attachment {
    pub pos: [i32; 3],
    pub radius_offset: i32,
    pub double: bool,
}

const FLAGS: u32 = 19;

fn valid_tree_pos(r: &Region, p: [i32; 3]) -> bool {
    let b = r.b;
    let s = r.get_p(p);
    b.is_air(s) || b.in_tag(s, b.rules.replaceable_by_trees)
}

fn is_free(r: &Region, t: &Tree, p: [i32; 3]) -> bool {
    trunk_valid(r, t, p) || r.b.in_tag(r.get_p(p), r.b.rules.logs)
}

fn trunk_valid(r: &Region, t: &Tree, p: [i32; 3]) -> bool {
    if valid_tree_pos(r, p) {
        return true;
    }
    if let TrunkKind::Upwards { through, .. } = &t.trunk.kind {
        return through.contains(r.b.block(r.get_p(p)));
    }
    false
}

struct Placer<'t> {
    t: &'t Tree,
}

impl Placer<'_> {
    fn set_trunk(&self, cx: &mut Ctx, p: [i32; 3], s: State) {
        cx.tree.trunks.insert(p);
        cx.r.set_p(p, s, FLAGS);
    }

    fn below_trunk(&self, cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3]) {
        if let Some(s) = self.t.below_trunk.get_opt(cx.r, rng, p) {
            self.set_trunk(cx, p, s);
        }
    }

    /// Placement of a log if the position is valid; `axis` forces the axis if given.
    fn log(&self, cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3], axis: Option<u8>) -> bool {
        if !trunk_valid(cx.r, self.t, p) {
            return false;
        }
        let b = cx.r.b;
        let mut s = self.t.trunk_provider.get(cx.r, rng, p);
        if let Some(a) = axis {
            s = b.with_axis(s, b.p.axis, a);
        }
        self.set_trunk(cx, p, s);
        true
    }

    fn log_if_free(&self, cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3]) {
        if is_free(cx.r, self.t, p) {
            self.log(cx, rng, p, None);
        }
    }

    fn leaf(&self, cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3]) -> bool {
        let r = &*cx.r;
        let b = r.b;
        let cur = r.get_p(p);
        if b.get_bool(cur, b.p.persistent) == Some(true) || !valid_tree_pos(r, p) {
            return false;
        }
        let mut s = self.t.foliage_provider.get(r, rng, p);
        if b.has(s, b.p.waterlogged) {
            s = b.with_bool(s, b.p.waterlogged, b.water_source(cur));
        }
        cx.tree.foliage.insert(p);
        cx.r.set_p(p, s, FLAGS);
        true
    }
}

fn random_horizontal(rng: &mut DecoRng) -> Dir {
    Dir::NESW[rng.next_int_bounded(4) as usize]
}

fn shuffled_dirs(rng: &mut DecoRng) -> [Dir; 6] {
    let mut d = Dir::ALL;
    rng.shuffle(&mut d);
    d
}

fn above(p: [i32; 3], n: i32) -> [i32; 3] {
    [p[0], p[1] + n, p[2]]
}

pub fn place_tree(cx: &mut Ctx, t: &Tree, rng: &mut DecoRng, origin: [i32; 3]) -> bool {
    {
        let s = &mut cx.tree;
        s.roots.clear();
        s.trunks.clear();
        s.foliage.clear();
        s.decorations.clear();
    }
    let pl = Placer { t };
    let ok = do_place(cx, &pl, rng, origin);
    if !ok || (cx.tree.trunks.is_empty() && cx.tree.foliage.is_empty()) {
        return false;
    }
    if !t.decorators.is_empty() {
        {
            let s = &mut cx.tree;
            s.trunks.sorted_by_y(&mut s.logs);
            s.foliage.sorted_by_y(&mut s.leaves);
            s.roots.sorted_by_y(&mut s.root_list);
        }
        for d in &t.decorators {
            run_decorator(cx, d, rng);
        }
    }
    update_leaves(cx);
    true
}

fn tree_height(t: &Trunk, rng: &mut DecoRng) -> i32 {
    let a = rng.next_int_bounded(t.a + 1);
    t.base + a + rng.next_int_bounded(t.b + 1)
}

fn foliage_height(f: &Foliage, rng: &mut DecoRng, tree_height: i32) -> i32 {
    match &f.kind {
        FoliageKind::Blob(h) | FoliageKind::Bush(h) | FoliageKind::Fancy(h) | FoliageKind::Jungle(h) => *h,
        FoliageKind::Spruce(th) => 4.max(tree_height - th.sample(rng)),
        FoliageKind::Pine(h) | FoliageKind::MegaPine(h) => h.sample(rng),
        FoliageKind::Acacia => 0,
        FoliageKind::DarkOak => 4,
        FoliageKind::Cherry { height, .. } | FoliageKind::Poplar { height, .. } => height.sample(rng),
        FoliageKind::RandomSpread { height, .. } => height.sample(rng),
    }
}

fn foliage_radius(f: &Foliage, rng: &mut DecoRng, trunk_height: i32) -> i32 {
    let r = f.radius.sample(rng);
    if let FoliageKind::Pine(_) = f.kind { r + rng.next_int_bounded((trunk_height + 1).max(1)) } else { r }
}

fn size_at(t: &Tree, h: i32, y: i32) -> i32 {
    let (limit, lower, middle, upper, upper_limit, three) = t.size;
    if y < limit {
        lower
    } else if three {
        if y >= h - upper_limit { upper } else { middle }
    } else {
        upper
    }
}

fn do_place(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, origin: [i32; 3]) -> bool {
    let t = pl.t;
    let h = tree_height(&t.trunk, rng);
    let fh = foliage_height(&t.foliage, rng, h);
    let trunk_h = h - fh;
    let leaf_r = foliage_radius(&t.foliage, rng, trunk_h);
    let trunk_origin = match &t.roots {
        Some(rp) => above(origin, rp.trunk_offset_y.sample(rng)),
        None => origin,
    };
    let min_y = origin[1].min(trunk_origin[1]);
    let max_y = origin[1].max(trunk_origin[1]) + h + 1;
    if min_y < cx.r.min_y + 1 || max_y > cx.r.max_y() + 1 {
        return false;
    }
    // Free height.
    let mut clipped = h;
    'outer: for y in 0..=h + 1 {
        let rr = size_at(t, h, y);
        for x in -rr..=rr {
            for z in -rr..=rr {
                let p = [trunk_origin[0] + x, trunk_origin[1] + y, trunk_origin[2] + z];
                if !is_free(cx.r, t, p) || (!t.ignore_vines && cx.r.b.is(cx.r.get_p(p), cx.r.b.k.vine)) {
                    clipped = y - 2;
                    break 'outer;
                }
            }
        }
    }
    if !(clipped >= h || t.min_clipped.is_some_and(|m| clipped >= m)) {
        return false;
    }
    if let Some(rp) = &t.roots
        && !place_mangrove_roots(cx, rp, rng, origin, trunk_origin)
    {
        return false;
    }
    let mut att = std::mem::take(&mut cx.tree.attach);
    att.clear();
    place_trunk(cx, pl, rng, clipped, trunk_origin, &mut att);
    for a in &att {
        create_foliage(cx, pl, rng, clipped, *a, fh, leaf_r);
    }
    cx.tree.attach = att;
    true
}

// ---------------------------------------------------------------- Trunks

fn place_trunk(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, h: i32, o: [i32; 3], att: &mut Vec<Attachment>) {
    let t = pl.t;
    match &t.trunk.kind {
        TrunkKind::Straight => {
            pl.below_trunk(cx, rng, above(o, -1));
            for y in 0..h {
                pl.log(cx, rng, above(o, y), None);
            }
            att.push(Attachment { pos: above(o, h), radius_offset: 0, double: false });
        }
        TrunkKind::Forking => {
            pl.below_trunk(cx, rng, above(o, -1));
            let lean = random_horizontal(rng);
            let lean_h = h - rng.next_int_bounded(4) - 1;
            let mut lean_steps = 3 - rng.next_int_bounded(3);
            let (mut tx, mut tz) = (o[0], o[2]);
            let mut ey = None;
            for yo in 0..h {
                let yy = o[1] + yo;
                if yo >= lean_h && lean_steps > 0 {
                    let off = lean.offset();
                    tx += off[0];
                    tz += off[2];
                    lean_steps -= 1;
                }
                if pl.log(cx, rng, [tx, yy, tz], None) {
                    ey = Some(yy + 1);
                }
            }
            if let Some(e) = ey {
                att.push(Attachment { pos: [tx, e, tz], radius_offset: 1, double: false });
            }
            tx = o[0];
            tz = o[2];
            let branch = random_horizontal(rng);
            if branch != lean {
                let branch_pos = lean_h - rng.next_int_bounded(2) - 1;
                let mut steps = 1 + rng.next_int_bounded(3);
                ey = None;
                let mut yo = branch_pos;
                while yo < h && steps > 0 {
                    if yo >= 1 {
                        let yy = o[1] + yo;
                        let off = branch.offset();
                        tx += off[0];
                        tz += off[2];
                        if pl.log(cx, rng, [tx, yy, tz], None) {
                            ey = Some(yy + 1);
                        }
                    }
                    yo += 1;
                    steps -= 1;
                }
                if let Some(e) = ey {
                    att.push(Attachment { pos: [tx, e, tz], radius_offset: 0, double: false });
                }
            }
        }
        TrunkKind::Giant | TrunkKind::MegaJungle => {
            let below = above(o, -1);
            pl.below_trunk(cx, rng, below);
            pl.below_trunk(cx, rng, add(below, Dir::East));
            pl.below_trunk(cx, rng, add(below, Dir::South));
            pl.below_trunk(cx, rng, add(add(below, Dir::South), Dir::East));
            for hh in 0..h {
                pl.log_if_free(cx, rng, [o[0], o[1] + hh, o[2]]);
                if hh < h - 1 {
                    pl.log_if_free(cx, rng, [o[0] + 1, o[1] + hh, o[2]]);
                    pl.log_if_free(cx, rng, [o[0] + 1, o[1] + hh, o[2] + 1]);
                    pl.log_if_free(cx, rng, [o[0], o[1] + hh, o[2] + 1]);
                }
            }
            att.push(Attachment { pos: above(o, h), radius_offset: 0, double: true });
            if let TrunkKind::MegaJungle = t.trunk.kind {
                let mut bh = h - 2 - rng.next_int_bounded(4);
                while bh > h / 2 {
                    let angle = rng.next_float() * (std::f64::consts::PI * 2.0) as f32;
                    let (mut bx, mut bz) = (0, 0);
                    for bb in 0..5 {
                        bx = (1.5f32 + cx.d.sin.cos(f64::from(angle)) * bb as f32) as i32;
                        bz = (1.5f32 + cx.d.sin.sin(f64::from(angle)) * bb as f32) as i32;
                        pl.log(cx, rng, [o[0] + bx, o[1] + bh - 3 + bb / 2, o[2] + bz], None);
                    }
                    att.push(Attachment { pos: [o[0] + bx, o[1] + bh, o[2] + bz], radius_offset: -2, double: false });
                    bh -= 2 + rng.next_int_bounded(4);
                }
            }
        }
        TrunkKind::DarkOak => {
            let below = above(o, -1);
            pl.below_trunk(cx, rng, below);
            pl.below_trunk(cx, rng, add(below, Dir::East));
            pl.below_trunk(cx, rng, add(below, Dir::South));
            pl.below_trunk(cx, rng, add(add(below, Dir::South), Dir::East));
            let lean = random_horizontal(rng);
            let lean_h = h - rng.next_int_bounded(4);
            let mut lean_steps = 2 - rng.next_int_bounded(3);
            let (x, y, z) = (o[0], o[1], o[2]);
            let (mut tx, mut tz) = (x, z);
            let ey = y + h - 1;
            for dy in 0..h {
                if dy >= lean_h && lean_steps > 0 {
                    let off = lean.offset();
                    tx += off[0];
                    tz += off[2];
                    lean_steps -= 1;
                }
                let p = [tx, y + dy, tz];
                let s = cx.r.get_p(p);
                let b = cx.r.b;
                if b.is_air(s) || b.in_tag(s, b.rules.leaves) {
                    pl.log(cx, rng, p, None);
                    pl.log(cx, rng, add(p, Dir::East), None);
                    pl.log(cx, rng, add(p, Dir::South), None);
                    pl.log(cx, rng, add(add(p, Dir::East), Dir::South), None);
                }
            }
            att.push(Attachment { pos: [tx, ey, tz], radius_offset: 0, double: true });
            for ox in -1..=2 {
                for oz in -1..=2 {
                    if (!(0..=1).contains(&ox) || !(0..=1).contains(&oz)) && rng.next_int_bounded(3) <= 0 {
                        let len = rng.next_int_bounded(3) + 2;
                        for by in 0..len {
                            pl.log(cx, rng, [x + ox, ey - by - 1, z + oz], None);
                        }
                        att.push(Attachment { pos: [x + ox, ey, z + oz], radius_offset: 0, double: false });
                    }
                }
            }
        }
        TrunkKind::Fancy => fancy_trunk(cx, pl, rng, h, o, att),
        TrunkKind::Bending { min_leaves, bend } => {
            let dir = random_horizontal(rng);
            let log_h = h - 1;
            let mut p = o;
            pl.below_trunk(cx, rng, above(p, -1));
            for i in 0..=log_h {
                if i + 1 >= log_h + rng.next_int_bounded(2) {
                    p = add(p, dir);
                }
                if valid_tree_pos(cx.r, p) {
                    pl.log(cx, rng, p, None);
                }
                if i >= *min_leaves {
                    att.push(Attachment { pos: p, radius_offset: 0, double: false });
                }
                p = add(p, Dir::Up);
            }
            let len = bend.sample(rng);
            for _ in 0..=len {
                if valid_tree_pos(cx.r, p) {
                    pl.log(cx, rng, p, None);
                }
                att.push(Attachment { pos: p, radius_offset: 0, double: false });
                p = add(p, dir);
            }
        }
        TrunkKind::Upwards { steps, prob, len, .. } => {
            for hp in 0..h {
                let cy = o[1] + hp;
                let lp = [o[0], cy, o[2]];
                if pl.log(cx, rng, lp, None) && hp < h - 1 && rng.next_float() < *prob {
                    let d = random_horizontal(rng);
                    let bl = len.sample(rng);
                    let bp = (bl - len.sample(rng) - 1).max(0);
                    let mut bs = steps.sample(rng);
                    // Branch.
                    let mut ha = cy + bp;
                    let (mut lx, mut lz) = (lp[0], lp[2]);
                    let mut i = bp;
                    while i < h && bs > 0 {
                        if i >= 1 {
                            let ph = cy + i;
                            let off = d.offset();
                            lx += off[0];
                            lz += off[2];
                            ha = ph;
                            if pl.log(cx, rng, [lx, ph, lz], None) {
                                ha += 1;
                            }
                            att.push(Attachment { pos: [lx, ph, lz], radius_offset: 0, double: false });
                        }
                        i += 1;
                        bs -= 1;
                    }
                    if ha - cy > 1 {
                        att.push(Attachment { pos: [lx, ha, lz], radius_offset: 0, double: false });
                        att.push(Attachment { pos: [lx, ha - 2, lz], radius_offset: 0, double: false });
                    }
                }
                if hp == h - 1 {
                    att.push(Attachment { pos: [o[0], cy + 1, o[2]], radius_offset: 0, double: false });
                }
            }
        }
        TrunkKind::Cherry { count, hlen, start, end } => {
            pl.below_trunk(cx, rng, above(o, -1));
            let first = 0.max(h - 1 + rng.between(start.0, start.1));
            let mut second = 0.max(h - 1 + rng.between(start.0, start.1 - 1));
            if second >= first {
                second += 1;
            }
            let n = count.sample(rng);
            let middle = n == 3;
            let both = n >= 2;
            let th = if middle {
                h
            } else if both {
                first.max(second) + 1
            } else {
                first + 1
            };
            for y in 0..th {
                pl.log(cx, rng, above(o, y), None);
            }
            if middle {
                att.push(Attachment { pos: above(o, th), radius_offset: 0, double: false });
            }
            let dir = random_horizontal(rng);
            let a1 = cherry_branch(cx, pl, rng, h, o, dir, first, first < th - 1, hlen, end);
            att.push(a1);
            if both {
                let a2 = cherry_branch(cx, pl, rng, h, o, dir.opposite(), second, second < th - 1, hlen, end);
                att.push(a2);
            }
        }
        TrunkKind::Poplar { above: ab, amount } => {
            pl.below_trunk(cx, rng, above(o, -1));
            let hb = h - ab.sample(rng);
            for y in 0..h {
                pl.log(cx, rng, above(o, y), None);
                let dirs = shuffled_dirs(rng);
                if hb - 1 == y {
                    let n = amount.sample(rng);
                    let horiz: Vec<Dir> = dirs.iter().copied().filter(|d| d.is_horizontal()).collect();
                    for &d in horiz.iter().take(n as usize) {
                        pl.log(cx, rng, add(above(o, y), d), Some(d.axis()));
                    }
                }
            }
            att.push(Attachment { pos: above(o, hb), radius_offset: 0, double: false });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cherry_branch(
    cx: &mut Ctx,
    pl: &Placer,
    rng: &mut DecoRng,
    h: i32,
    o: [i32; 3],
    dir: Dir,
    off: i32,
    continues: bool,
    hlen: &IntP,
    end: &IntP,
) -> Attachment {
    let mut lp = above(o, off);
    let end_off = h - 1 + end.sample(rng);
    let away = continues || end_off < off;
    let dist = hlen.sample(rng) + i32::from(away);
    let end_pos = above(add_n(o, dir, dist), end_off);
    let steps = if away { 2 } else { 1 };
    for _ in 0..steps {
        lp = add(lp, dir);
        pl.log(cx, rng, lp, Some(dir.axis()));
    }
    let vert = if end_pos[1] > lp[1] { Dir::Up } else { Dir::Down };
    loop {
        let d = (lp[0] - end_pos[0]).abs() + (lp[1] - end_pos[1]).abs() + (lp[2] - end_pos[2]).abs();
        if d == 0 {
            return Attachment { pos: above(end_pos, 1), radius_offset: 0, double: false };
        }
        let chance = (end_pos[1] - lp[1]).abs() as f32 / d as f32;
        let grow_v = rng.next_float() < chance;
        lp = add(lp, if grow_v { vert } else { dir });
        pl.log(cx, rng, lp, if grow_v { None } else { Some(dir.axis()) });
    }
}

fn fancy_trunk(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, tree_h: i32, o: [i32; 3], att: &mut Vec<Attachment>) {
    let height = tree_h + 2;
    let trunk_h = (f64::from(height) * 0.618).floor() as i32;
    pl.below_trunk(cx, rng, above(o, -1));
    let clusters = 1.min((1.382 + (f64::from(height) / 13.0).powf(2.0)).floor() as i32);
    let trunk_top = o[1] + trunk_h;
    let mut rel_y = height - 5;
    // (pos, base)
    let mut coords: Vec<([i32; 3], i32)> = vec![(above(o, rel_y), trunk_top)];
    while rel_y >= 0 {
        let shape = tree_shape(height, rel_y);
        if shape >= 0.0 {
            for _ in 0..clusters {
                let radius = f64::from(shape) * (f64::from(rng.next_float()) + 0.328);
                let angle = f64::from(rng.next_float() * 2.0) * std::f64::consts::PI;
                let x = radius * crate::jmath::sin(angle) + 0.5;
                let z = radius * crate::jmath::cos(angle) + 0.5;
                let start = [o[0] + x.floor() as i32, o[1] + rel_y - 1, o[2] + z.floor() as i32];
                let end = above(start, 5);
                if make_limb(cx, pl, rng, start, end, false) {
                    let dx = o[0] - start[0];
                    let dz = o[2] - start[2];
                    let bh = f64::from(start[1]) - f64::from(dx * dx + dz * dz).sqrt() * 0.381;
                    let top = if bh > f64::from(trunk_top) { trunk_top } else { bh as i32 };
                    let base = [o[0], top, o[2]];
                    if make_limb(cx, pl, rng, base, start, false) {
                        coords.push((start, base[1]));
                    }
                }
            }
        }
        rel_y -= 1;
    }
    make_limb(cx, pl, rng, o, above(o, trunk_h), true);
    for &(pos, base) in &coords {
        let bc = [o[0], base, o[2]];
        if bc != pos && f64::from(base - o[1]) >= f64::from(height) * 0.2 {
            make_limb(cx, pl, rng, bc, pos, true);
        }
    }
    for &(pos, base) in &coords {
        if f64::from(base - o[1]) >= f64::from(height) * 0.2 {
            att.push(Attachment { pos, radius_offset: 0, double: false });
        }
    }
}

fn tree_shape(height: i32, y: i32) -> f32 {
    if (y as f32) < height as f32 * 0.3 {
        return -1.0;
    }
    let radius = height as f32 / 2.0;
    let adjacent = radius - y as f32;
    let mut distance = (radius * radius - adjacent * adjacent).sqrt();
    if adjacent == 0.0 {
        distance = radius;
    } else if adjacent.abs() >= radius {
        return 0.0;
    }
    distance * 0.5
}

fn make_limb(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, start: [i32; 3], end: [i32; 3], place: bool) -> bool {
    if !place && start == end {
        return true;
    }
    let d = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
    let steps = d[0].abs().max(d[1].abs().max(d[2].abs()));
    let dx = d[0] as f32 / steps as f32;
    let dy = d[1] as f32 / steps as f32;
    let dz = d[2] as f32 / steps as f32;
    for i in 0..=steps {
        let p = [
            start[0] + (0.5f32 + i as f32 * dx).floor() as i32,
            start[1] + (0.5f32 + i as f32 * dy).floor() as i32,
            start[2] + (0.5f32 + i as f32 * dz).floor() as i32,
        ];
        if place {
            let xd = (p[0] - start[0]).abs();
            let zd = (p[2] - start[2]).abs();
            let m = xd.max(zd);
            let axis = if m > 0 { if xd == m { 0 } else { 2 } } else { 1 };
            pl.log(cx, rng, p, Some(axis));
        } else if !is_free(cx.r, pl.t, p) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------- Foliage

fn create_foliage(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, tree_h: i32, a: Attachment, fh: i32, leaf_r: i32) {
    let f = &pl.t.foliage;
    let offset = f.offset.sample(rng);
    let dbl = a.double;
    match &f.kind {
        FoliageKind::Blob(_) | FoliageKind::Bush(_) => {
            let bush = matches!(f.kind, FoliageKind::Bush(_));
            let mut yo = offset;
            while yo >= offset - fh {
                let cr = if bush {
                    leaf_r + a.radius_offset - 1 - yo
                } else {
                    (leaf_r + a.radius_offset - 1 - yo / 2).max(0)
                };
                leaves_row(cx, pl, rng, a.pos, cr, yo, dbl);
                yo -= 1;
            }
        }
        FoliageKind::Fancy(_) => {
            let mut yo = offset;
            while yo >= offset - fh {
                let cr = leaf_r + i32::from(yo != offset && yo != offset - fh);
                leaves_row(cx, pl, rng, a.pos, cr, yo, dbl);
                yo -= 1;
            }
        }
        FoliageKind::Spruce(_) => {
            let mut cr = rng.next_int_bounded(2);
            let mut max_r = 1;
            let mut min_r = 0;
            let mut yo = offset;
            while yo >= -fh {
                leaves_row(cx, pl, rng, a.pos, cr, yo, dbl);
                if cr >= max_r {
                    cr = min_r;
                    min_r = 1;
                    max_r = (max_r + 1).min(leaf_r + a.radius_offset);
                } else {
                    cr += 1;
                }
                yo -= 1;
            }
        }
        FoliageKind::Pine(_) => {
            let mut cr = 0;
            let mut yo = offset;
            while yo >= offset - fh {
                leaves_row(cx, pl, rng, a.pos, cr, yo, dbl);
                if cr >= 1 && yo == offset - fh + 1 {
                    cr -= 1;
                } else if cr < leaf_r + a.radius_offset {
                    cr += 1;
                }
                yo -= 1;
            }
        }
        FoliageKind::MegaPine(_) => {
            let mut prev = 0;
            let mut yy = a.pos[1] - fh + offset;
            while yy <= a.pos[1] + offset {
                let yo = a.pos[1] - yy;
                let smooth = leaf_r + a.radius_offset + (yo as f32 / fh as f32 * 3.5).floor() as i32;
                let jagged = if yo > 0 && smooth == prev && yy & 1 == 0 { smooth + 1 } else { smooth };
                leaves_row(cx, pl, rng, [a.pos[0], yy, a.pos[2]], jagged, 0, dbl);
                prev = smooth;
                yy += 1;
            }
        }
        FoliageKind::Jungle(_) => {
            let lh = if dbl { fh } else { 1 + rng.next_int_bounded(2) };
            let mut yo = offset;
            while yo >= offset - lh {
                let cr = leaf_r + a.radius_offset + 1 - yo;
                leaves_row(cx, pl, rng, a.pos, cr, yo, dbl);
                yo -= 1;
            }
        }
        FoliageKind::Acacia => {
            let p = above(a.pos, offset);
            leaves_row(cx, pl, rng, p, leaf_r + a.radius_offset, -1 - fh, dbl);
            leaves_row(cx, pl, rng, p, leaf_r - 1, -fh, dbl);
            leaves_row(cx, pl, rng, p, leaf_r + a.radius_offset - 1, 0, dbl);
        }
        FoliageKind::DarkOak => {
            let p = above(a.pos, offset);
            if dbl {
                leaves_row(cx, pl, rng, p, leaf_r + 2, -1, dbl);
                leaves_row(cx, pl, rng, p, leaf_r + 3, 0, dbl);
                leaves_row(cx, pl, rng, p, leaf_r + 2, 1, dbl);
                if rng.next_bool() {
                    leaves_row(cx, pl, rng, p, leaf_r, 2, dbl);
                }
            } else {
                leaves_row(cx, pl, rng, p, leaf_r + 2, -1, dbl);
                leaves_row(cx, pl, rng, p, leaf_r + 1, 0, dbl);
            }
        }
        FoliageKind::Cherry { hanging, hanging_ext, .. } => {
            let p = above(a.pos, offset);
            let cr = leaf_r + a.radius_offset - 1;
            leaves_row(cx, pl, rng, p, cr - 2, fh - 3, dbl);
            leaves_row(cx, pl, rng, p, cr - 1, fh - 4, dbl);
            let mut y = fh - 5;
            while y >= 0 {
                leaves_row(cx, pl, rng, p, cr, y, dbl);
                y -= 1;
            }
            hanging_row(cx, pl, rng, p, cr, -1, dbl, *hanging, *hanging_ext);
            hanging_row(cx, pl, rng, p, cr - 1, -2, dbl, *hanging, *hanging_ext);
        }
        FoliageKind::RandomSpread { attempts, .. } => {
            for _ in 0..*attempts {
                let dx = rng.next_int_bounded(leaf_r) - rng.next_int_bounded(leaf_r);
                let dy = rng.next_int_bounded(fh) - rng.next_int_bounded(fh);
                let dz = rng.next_int_bounded(leaf_r) - rng.next_int_bounded(leaf_r);
                pl.leaf(cx, rng, [a.pos[0] + dx, a.pos[1] + dy, a.pos[2] + dz]);
            }
        }
        FoliageKind::Poplar { side_hole, .. } => {
            let p = above(a.pos, offset);
            let cr = leaf_r + a.radius_offset - 1;
            let flip = rng.next_bool();
            let fhh = fh;
            poplar_row(cx, pl, rng, p, cr - 2, fhh - 1, dbl, fhh, flip, *side_hole);
            poplar_row(cx, pl, rng, p, cr - 1, fhh - 2, dbl, fhh, flip, *side_hole);
            poplar_row(cx, pl, rng, p, cr - 1, fhh - 3, dbl, fhh, flip, *side_hole);
            let mut y = fhh - 4;
            while y >= 1 {
                poplar_row(cx, pl, rng, p, cr, y, dbl, fhh, flip, *side_hole);
                y -= 1;
            }
            poplar_logs(cx, pl, rng, p, cr, fhh - 4, dbl, fhh, flip);
            poplar_row(cx, pl, rng, p, cr - 1, 0, dbl, fhh, flip, *side_hole);
            poplar_row(cx, pl, rng, p, (cr - 2).clamp(1, 2), -1, dbl, fhh, flip, *side_hole);
        }
    }
    let _ = tree_h;
}

/// "Ignore this position" test of the placer (dx, dz already made absolute).
fn skip(f: &FoliageKind, rng: &mut DecoRng, dx: i32, y: i32, dz: i32, r: i32, dbl: bool) -> bool {
    match f {
        FoliageKind::Blob(_) => dx == r && dz == r && (rng.next_int_bounded(2) == 0 || y == 0),
        FoliageKind::Bush(_) => dx == r && dz == r && rng.next_int_bounded(2) == 0,
        FoliageKind::Fancy(_) => {
            let a = dx as f32 + 0.5;
            let c = dz as f32 + 0.5;
            a * a + c * c > (r * r) as f32
        }
        FoliageKind::Spruce(_) | FoliageKind::Pine(_) => dx == r && dz == r && r > 0,
        FoliageKind::MegaPine(_) | FoliageKind::Jungle(_) => dx + dz >= 7 || dx * dx + dz * dz > r * r,
        FoliageKind::Acacia => {
            if y == 0 {
                (dx > 1 || dz > 1) && dx != 0 && dz != 0
            } else {
                dx == r && dz == r && r > 0
            }
        }
        FoliageKind::DarkOak => {
            if y == -1 && !dbl {
                dx == r && dz == r
            } else if y == 1 {
                dx + dz > r * 2 - 2
            } else {
                false
            }
        }
        FoliageKind::Cherry { wide_hole, corner_hole, .. } => {
            if y == -1 && (dx == r || dz == r) && rng.next_float() < *wide_hole {
                return true;
            }
            let corner = dx == r && dz == r;
            if r > 2 {
                corner || (dx + dz > r * 2 - 2 && rng.next_float() < *corner_hole)
            } else {
                corner && rng.next_float() < *corner_hole
            }
        }
        FoliageKind::RandomSpread { .. } | FoliageKind::Poplar { .. } => false,
    }
}

fn leaves_row(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, o: [i32; 3], r: i32, y: i32, dbl: bool) {
    let f = &pl.t.foliage.kind;
    let off = i32::from(dbl);
    for dx in -r..=r + off {
        for dz in -r..=r + off {
            // Signed variant.
            let skip_it =
                if matches!(f, FoliageKind::DarkOak) && y == 0 && dbl && (dx == -r || dx >= r) && (dz == -r || dz >= r)
                {
                    true
                } else {
                    let (mx, mz) = if dbl {
                        (dx.abs().min((dx - 1).abs()), dz.abs().min((dz - 1).abs()))
                    } else {
                        (dx.abs(), dz.abs())
                    };
                    rng.label = 1;
                    let s = skip(f, rng, mx, y, mz, r, dbl);
                    rng.label = 0;
                    s
                };
            if !skip_it {
                pl.leaf(cx, rng, [o[0] + dx, o[1] + y, o[2] + dz]);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn hanging_row(
    cx: &mut Ctx,
    pl: &Placer,
    rng: &mut DecoRng,
    o: [i32; 3],
    r: i32,
    y: i32,
    dbl: bool,
    chance: f32,
    ext: f32,
) {
    leaves_row(cx, pl, rng, o, r, y, dbl);
    let off = i32::from(dbl);
    let log_pos = above(o, -1);
    for along in Dir::NESW {
        let to_edge = along.cw();
        let ote = if to_edge.positive() { r + off } else { r };
        let mut p = add_n(add_n(above(o, y - 1), to_edge, ote), along, -r);
        let mut k = -r;
        while k < r + off {
            let leaves_above = cx.tree.foliage.contains(above(p, 1));
            if leaves_above && try_extension(cx, pl, rng, chance, log_pos, p) {
                try_extension(cx, pl, rng, ext, log_pos, above(p, -1));
            }
            k += 1;
            p = add(p, along);
        }
    }
}

fn try_extension(cx: &mut Ctx, pl: &Placer, rng: &mut DecoRng, chance: f32, log_pos: [i32; 3], p: [i32; 3]) -> bool {
    let d = (p[0] - log_pos[0]).abs() + (p[1] - log_pos[1]).abs() + (p[2] - log_pos[2]).abs();
    if d >= 7 {
        return false;
    }
    rng.label = 2;
    let v = rng.next_float();
    rng.label = 0;
    if v > chance {
        return false;
    }
    pl.leaf(cx, rng, p)
}

fn poplar_corner_cut(dx: i32, dz: i32, r: i32, partial: bool, flip: bool) -> i32 {
    let small = if flip { (dx > 0 && dz > 0) || (dz < 0 && dx < 0) } else { (dx > 0 && dz < 0) || (dz > 0 && dx < 0) };
    if small {
        r - 1
    } else if partial {
        r + 1
    } else {
        r
    }
}

#[allow(clippy::too_many_arguments)]
fn poplar_row(
    cx: &mut Ctx,
    pl: &Placer,
    rng: &mut DecoRng,
    o: [i32; 3],
    r: i32,
    y: i32,
    dbl: bool,
    fh: i32,
    flip: bool,
    side_hole: f32,
) {
    let off = i32::from(dbl);
    for dx in -r..=r + off {
        for dz in -r..=r + off {
            let partial = fh - 1 == y || fh - 2 == y;
            let cut = poplar_corner_cut(dx, dz, r, partial, flip);
            let (ax, az) = (dx.abs(), dz.abs());
            let edge = ax == r || az == r;
            let skip_it = if partial && edge {
                true
            } else {
                let extra = i32::from(rng.next_float() <= side_hole);
                ax + az > r * 2 - (cut + extra)
            };
            if !skip_it {
                pl.leaf(cx, rng, [o[0] + dx, o[1] + y, o[2] + dz]);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn poplar_logs(
    cx: &mut Ctx,
    pl: &Placer,
    rng: &mut DecoRng,
    o: [i32; 3],
    r: i32,
    y: i32,
    dbl: bool,
    fh: i32,
    flip: bool,
) {
    let off = i32::from(dbl);
    for dx in -r..=r + off {
        for dz in -r..=r + off {
            let (ax, az) = (dx.abs(), dz.abs());
            let partial = fh - 1 == y || fh - 2 == y;
            let cut = poplar_corner_cut(dx, dz, r, partial, flip);
            if ax + az <= r * 2 - (cut + 2) && ((az == 0 && r - ax >= 4) || (ax == 0 && r - az >= 4)) {
                let p = [o[0] + dx, o[1] + y, o[2] + dz];
                let cur = cx.r.get_p(p);
                let fs = pl.t.foliage_provider.get(cx.r, rng, p);
                if cur == fs {
                    let b = cx.r.b;
                    let mut s = pl.t.trunk_provider.get(cx.r, rng, p);
                    s = b.with_str(s, b.p.axis, if az == 0 { "x" } else { "z" });
                    cx.tree.foliage.insert(p);
                    cx.r.set_p(p, s, FLAGS);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- Mangrove roots

fn root_possible(r: &Region, rp: &MangroveRoots, p: [i32; 3]) -> bool {
    valid_tree_pos(r, p) || rp.can_grow_through.contains(r.b.block(r.get_p(p)))
}

fn place_mangrove_roots(cx: &mut Ctx, rp: &MangroveRoots, rng: &mut DecoRng, origin: [i32; 3], t: [i32; 3]) -> bool {
    let mut y = origin[1];
    while y < t[1] {
        if !root_possible(cx.r, rp, [origin[0], y, origin[2]]) {
            return false;
        }
        y += 1;
    }
    let mut list = vec![above(t, -1)];
    for d in Dir::NESW {
        let p = add(t, d);
        let mut l = Vec::new();
        if !simulate_roots(cx.r, rp, rng, p, d, t, &mut l, 0) {
            return false;
        }
        list.extend_from_slice(&l);
        list.push(add(t, d));
    }
    let b = cx.r.b;
    for p in list {
        let cur = cx.r.get_p(p);
        if rp.muddy_in.contains(b.block(cur)) {
            let s = rp.muddy_provider.get(cx.r, rng, p);
            let s = waterlog_if_water(cx.r, p, s);
            cx.tree.roots.insert(p);
            cx.r.set_p(p, s, FLAGS);
        } else if root_possible(cx.r, rp, p) {
            let s = rp.provider.get(cx.r, rng, p);
            let s = waterlog_if_water(cx.r, p, s);
            cx.tree.roots.insert(p);
            cx.r.set_p(p, s, FLAGS);
            if let Some((ap, chance)) = &rp.above {
                let up = above(p, 1);
                if rng.next_float() < *chance && b.is_air(cx.r.get_p(up)) {
                    let s = ap.get(cx.r, rng, up);
                    let s = waterlog_if_water(cx.r, up, s);
                    cx.tree.roots.insert(up);
                    cx.r.set_p(up, s, FLAGS);
                }
            }
        }
    }
    true
}

fn waterlog_if_water(r: &Region, p: [i32; 3], s: State) -> State {
    let b = r.b;
    if b.has(s, b.p.waterlogged) { b.with_bool(s, b.p.waterlogged, b.fluid(r.get_p(p)) == 1) } else { s }
}

#[allow(clippy::too_many_arguments)]
fn simulate_roots(
    r: &Region,
    rp: &MangroveRoots,
    rng: &mut DecoRng,
    p: [i32; 3],
    d: Dir,
    origin: [i32; 3],
    out: &mut Vec<[i32; 3]>,
    layer: i32,
) -> bool {
    if layer == rp.max_length || out.len() as i32 > rp.max_length {
        return false;
    }
    let below = above(p, -1);
    let next = add(p, d);
    let width = (p[0] - origin[0]).abs() + (p[1] - origin[1]).abs() + (p[2] - origin[2]).abs();
    let mut cand = [[0; 3]; 2];
    let n = if width > rp.max_width - 3 && width <= rp.max_width {
        if rng.next_float() < rp.skew {
            cand = [below, above(next, -1)];
            2
        } else {
            cand[0] = below;
            1
        }
    } else if width > rp.max_width || rng.next_float() < rp.skew {
        cand[0] = below;
        1
    } else {
        cand[0] = if rng.next_bool() { next } else { below };
        1
    };
    for &q in &cand[..n] {
        if root_possible(r, rp, q) {
            out.push(q);
            if !simulate_roots(r, rp, rng, q, d, origin, out, layer + 1) {
                return false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------- Decorators

fn set_decoration(cx: &mut Ctx, p: [i32; 3], s: State) {
    cx.tree.decorations.insert(p);
    cx.r.set_p(p, s, FLAGS);
}

fn vine_state(b: &Blocks, face: Dir) -> State {
    let v = b.default_state(b.k.vine);
    b.with_bool(v, crate::decor_rules::face_prop(b, face), true)
}

/// "Bottom" positions (roots and/or lowest logs).
fn lowest(cx: &Ctx, out: &mut Vec<[i32; 3]>) {
    let s = &cx.tree;
    out.clear();
    if s.root_list.is_empty() {
        out.extend_from_slice(&s.logs);
    } else if !s.logs.is_empty() && s.root_list[0][1] == s.logs[0][1] {
        out.extend_from_slice(&s.logs);
        out.extend_from_slice(&s.root_list);
    } else {
        out.extend_from_slice(&s.root_list);
    }
}

fn run_decorator(cx: &mut Ctx, d: &TreeDecorator, rng: &mut DecoRng) {
    let b = cx.r.b;
    match d {
        TreeDecorator::TrunkVine => {
            let logs = copy_list(&mut cx.tree, logs_of);
            for &p in &logs {
                for (side, face) in
                    [(Dir::West, Dir::East), (Dir::East, Dir::West), (Dir::North, Dir::South), (Dir::South, Dir::North)]
                {
                    if rng.next_int_bounded(3) > 0 {
                        let q = add(p, side);
                        if b.is_air(cx.r.get_p(q)) {
                            set_decoration(cx, q, vine_state(b, face));
                        }
                    }
                }
            }
            recycle(&mut cx.tree, logs);
        }
        TreeDecorator::LeaveVine(prob) => {
            let leaves = copy_list(&mut cx.tree, leaves_of);
            for &p in &leaves {
                for (side, face) in
                    [(Dir::West, Dir::East), (Dir::East, Dir::West), (Dir::North, Dir::South), (Dir::South, Dir::North)]
                {
                    if rng.next_float() < *prob {
                        let q = add(p, side);
                        if b.is_air(cx.r.get_p(q)) {
                            let v = vine_state(b, face);
                            set_decoration(cx, q, v);
                            let mut c = above(q, -1);
                            let mut n = 4;
                            while b.is_air(cx.r.get_p(c)) && n > 0 {
                                set_decoration(cx, c, v);
                                c = above(c, -1);
                                n -= 1;
                            }
                        }
                    }
                }
            }
            recycle(&mut cx.tree, leaves);
        }
        TreeDecorator::Cocoa(prob) => {
            if rng.next_float() >= *prob {
                return;
            }
            let logs = copy_list(&mut cx.tree, logs_of);
            if logs.is_empty() {
                return;
            }
            let ty = logs[0][1];
            for p in logs.iter().filter(|p| p[1] - ty <= 2) {
                for d in Dir::NESW {
                    if rng.next_float() <= 0.25 {
                        let o = d.opposite().offset();
                        let q = [p[0] + o[0], p[1], p[2] + o[2]];
                        if b.is_air(cx.r.get_p(q)) {
                            let age = rng.next_int_bounded(3);
                            let s = b.default_state(b.k.cocoa);
                            let s = b.with_int(s, b.p.age, age);
                            let s = b.with_dir(s, b.p.facing, d);
                            set_decoration(cx, q, s);
                        }
                    }
                }
            }
            recycle(&mut cx.tree, logs);
        }
        TreeDecorator::Beehive(prob) => {
            let logs = copy_list(&mut cx.tree, logs_of);
            if logs.is_empty() {
                return;
            }
            if rng.next_float() >= *prob {
                return;
            }
            let leaves = &cx.tree.leaves;
            let hive_y = if !leaves.is_empty() {
                (leaves[0][1] - 1).max(logs[0][1] + 1)
            } else {
                (logs[0][1] + 1 + rng.next_int_bounded(3)).min(logs[logs.len() - 1][1])
            };
            let mut cand: Vec<[i32; 3]> = Vec::new();
            for p in logs.iter().filter(|p| p[1] == hive_y) {
                for d in [Dir::East, Dir::South, Dir::West] {
                    cand.push(add(*p, d));
                }
            }
            if cand.is_empty() {
                return;
            }
            rng.shuffle(&mut cand);
            let found =
                cand.iter().copied().find(|q| b.is_air(cx.r.get_p(*q)) && b.is_air(cx.r.get_p(add(*q, Dir::South))));
            if let Some(q) = found {
                let s = b.with_dir(b.default_state(b.k.bee_nest), b.p.facing, Dir::South);
                set_decoration(cx, q, s);
                if cx.r.has_block_entity(q[0], q[1], q[2]) {
                    let n = 2 + rng.next_int_bounded(2);
                    let ticks = (0..n).map(|_| rng.next_int_bounded(599)).collect();
                    cx.r.note_block_entity(q, BeData::Bees(ticks));
                }
            }
            recycle(&mut cx.tree, logs);
        }
        TreeDecorator::AlterGround(prov) => {
            let mut low = std::mem::take(&mut cx.tree.tmp);
            lowest(cx, &mut low);
            if !low.is_empty() {
                let min_y = low[0][1];
                for &p in low.iter().filter(|p| p[1] == min_y) {
                    for c in [
                        [p[0] - 1, p[1], p[2] - 1],
                        [p[0] + 2, p[1], p[2] - 1],
                        [p[0] - 1, p[1], p[2] + 2],
                        [p[0] + 2, p[1], p[2] + 2],
                    ] {
                        alter_circle(cx, prov, rng, c);
                    }
                    for _ in 0..5 {
                        let k = rng.next_int_bounded(64);
                        let (xx, zz) = (k % 8, k / 8);
                        if xx == 0 || xx == 7 || zz == 0 || zz == 7 {
                            alter_circle(cx, prov, rng, [p[0] - 3 + xx, p[1], p[2] - 3 + zz]);
                        }
                    }
                }
            }
            cx.tree.tmp = low;
        }
        TreeDecorator::AttachedToLeaves { prob, ex_xz, ex_y, provider, required, dirs } => {
            let mut leaves = copy_list(&mut cx.tree, leaves_of);
            rng.shuffle(&mut leaves);
            let mut black = if crate::opt::tree_take() { std::mem::take(&mut cx.tree.black) } else { Default::default() };
            black.clear();
            for &p in &leaves {
                let d = dirs[rng.next_int_bounded(dirs.len() as i32) as usize];
                let q = add(p, d);
                if !black.contains(&q)
                    && rng.next_float() < *prob
                    && (1..=*required).all(|i| b.is_air(cx.r.get_p(add_n(p, d, i))))
                {
                    for x in q[0] - ex_xz..=q[0] + ex_xz {
                        for y in q[1] - ex_y..=q[1] + ex_y {
                            for z in q[2] - ex_xz..=q[2] + ex_xz {
                                black.insert([x, y, z]);
                            }
                        }
                    }
                    let s = provider.get(cx.r, rng, q);
                    set_decoration(cx, q, s);
                }
            }
            recycle(&mut cx.tree, leaves);
            if crate::opt::tree_take() {
                cx.tree.black = black;
            }
        }
        TreeDecorator::AttachedToLogs { prob, provider, dirs } => {
            let mut logs = copy_list(&mut cx.tree, logs_of);
            rng.shuffle(&mut logs);
            for &p in &logs {
                let d = dirs[rng.next_int_bounded(dirs.len() as i32) as usize];
                let q = add(p, d);
                if rng.next_float() <= *prob && b.is_air(cx.r.get_p(q)) {
                    let s = provider.get(cx.r, rng, q);
                    set_decoration(cx, q, s);
                }
            }
            recycle(&mut cx.tree, logs);
        }
        TreeDecorator::PlaceOnGround { tries, radius, height, provider } => {
            let mut low = std::mem::take(&mut cx.tree.tmp);
            lowest(cx, &mut low);
            if !low.is_empty() {
                let o = low[0];
                let min_y = o[1];
                let (mut min_x, mut max_x, mut min_z, mut max_z) = (o[0], o[0], o[2], o[2]);
                for p in low.iter().filter(|p| p[1] == min_y) {
                    min_x = min_x.min(p[0]);
                    max_x = max_x.max(p[0]);
                    min_z = min_z.min(p[2]);
                    max_z = max_z.max(p[2]);
                }
                let (x0, x1) = (min_x - radius, max_x + radius);
                let (y0, y1) = (min_y - height, min_y + height);
                let (z0, z1) = (min_z - radius, max_z + radius);
                for _ in 0..*tries {
                    let x = rng.between(x0, x1);
                    let y = rng.between(y0, y1);
                    let z = rng.between(z0, z1);
                    let p = [x, y, z];
                    let up = above(p, 1);
                    let us = cx.r.get_p(up);
                    if (b.is_air(us) || b.is(us, b.k.vine))
                        && b.solid_render(cx.r.get_p(p))
                        && cx.r.height_at(Hm::MotionBlockingNoLeaves, x, z) <= up[1]
                    {
                        let s = provider.get(cx.r, rng, up);
                        set_decoration(cx, up, s);
                    }
                }
            }
            cx.tree.tmp = low;
        }
        TreeDecorator::PaleMoss { leaves, trunk, ground } => {
            let mut logs = copy_list(&mut cx.tree, logs_of);
            rng.shuffle(&mut logs);
            if logs.is_empty() {
                return;
            }
            let mut origin = logs[0];
            for p in &logs {
                if p[1] < origin[1] {
                    origin = *p;
                }
            }
            if rng.next_float() < *ground {
                crate::feature_veg::place_pale_moss_patch(cx, rng, above(origin, 1));
            }
            recycle(&mut cx.tree, logs);
            let sorted_logs = copy_list(&mut cx.tree, logs_of);
            for &p in &sorted_logs {
                if rng.next_float() < *trunk {
                    let down = above(p, -1);
                    if b.is_air(cx.r.get_p(down)) {
                        moss_hanger(cx, rng, down);
                    }
                }
            }
            recycle(&mut cx.tree, sorted_logs);
            let lv = copy_list(&mut cx.tree, leaves_of);
            for &p in &lv {
                if rng.next_float() < *leaves {
                    let down = above(p, -1);
                    if b.is_air(cx.r.get_p(down)) {
                        moss_hanger(cx, rng, down);
                    }
                }
            }
            recycle(&mut cx.tree, lv);
        }
        TreeDecorator::CreakingHeart(prob) => {
            let logs = copy_list(&mut cx.tree, logs_of);
            if logs.is_empty() || rng.next_float() >= *prob {
                return;
            }
            let mut list = logs;
            rng.shuffle(&mut list);
            let logs_tag = b.rules.logs;
            let found =
                list.iter().copied().find(|p| Dir::ALL.iter().all(|d| b.in_tag(cx.r.get_p(add(*p, *d)), logs_tag)));
            if let Some(p) = found {
                let s = b.default_state(b.k.creaking_heart);
                let s = b.with_str(s, b.p.creaking_heart_state, "dormant");
                let s = b.with_bool(s, b.p.natural, true);
                set_decoration(cx, p, s);
            }
            recycle(&mut cx.tree, list);
        }
        TreeDecorator::ShelfMushroom(prob) => {
            if rng.next_float() >= *prob {
                return;
            }
            let logs = copy_list(&mut cx.tree, logs_of);
            if logs.is_empty() {
                return;
            }
            shelf_mushrooms(cx, rng, &logs);
            recycle(&mut cx.tree, logs);
        }
    }
}

fn alter_circle(cx: &mut Ctx, prov: &Provider, rng: &mut DecoRng, c: [i32; 3]) {
    for xx in -2i32..=2 {
        for zz in -2i32..=2 {
            if xx.abs() != 2 || zz.abs() != 2 {
                let p = [c[0] + xx, c[1], c[2] + zz];
                let mut dy = 2;
                while dy >= -3 {
                    let q = above(p, dy);
                    if let Some(s) = prov.get_opt(cx.r, rng, q) {
                        set_decoration(cx, q, s);
                        break;
                    }
                    if !cx.r.b.is_air(cx.r.get_p(q)) && dy < 0 {
                        break;
                    }
                    dy -= 1;
                }
            }
        }
    }
}

fn moss_hanger(cx: &mut Ctx, rng: &mut DecoRng, mut p: [i32; 3]) {
    let b = cx.r.b;
    let moss = b.default_state(b.k.pale_hanging_moss);
    while b.is_air(cx.r.get_p(above(p, -1))) && rng.next_float() >= 0.5 {
        set_decoration(cx, p, b.with_bool(moss, b.p.tip, false));
        p = above(p, -1);
    }
    set_decoration(cx, p, b.with_bool(moss, b.p.tip, true));
}

fn shelf_mushrooms(cx: &mut Ctx, rng: &mut DecoRng, logs: &[[i32; 3]]) {
    let b = cx.r.b;
    let shelf = if crate::opt::decor_ids() {
        Some(b.k.shelf_mushroom).filter(|&k| k != crate::decor_blocks::BlockId::MAX)
    } else {
        b.block_id("shelf_mushroom")
    };
    let Some(shelf) = shelf else { return };
    let water = b.k.water;
    let is_shelf = |r: &Region, p: [i32; 3]| b.is(r.get_p(p), shelf);
    let replaceable_dry = |r: &Region, p: [i32; 3]| {
        b.replaceable(r.get_p(p))
            && !(b.is(r.get_p(p), water)
                || b.is(r.get_p(add(p, Dir::East)), water)
                || b.is(r.get_p(add(p, Dir::West)), water)
                || b.is(r.get_p(add(p, Dir::North)), water)
                || b.is(r.get_p(add(p, Dir::South)), water))
    };
    let place = |cx: &mut Ctx, rng: &mut DecoRng, p: [i32; 3], facing: Dir| {
        let age = rng.next_int_bounded(2);
        let s = b.with_int(b.default_state(shelf), b.p.age, age);
        let s = b.with_dir(s, b.p.facing, facing);
        set_decoration(cx, p, s);
    };
    let fallen = logs[0][1] == logs[logs.len() - 1][1];
    if fallen {
        let (first, last) = (logs[0], logs[logs.len() - 1]);
        let dirs = if first[0] != last[0] { [Dir::North, Dir::South] } else { [Dir::East, Dir::West] };
        for &p in logs {
            for d in dirs {
                if rng.next_float() <= 0.25 {
                    let o = d.offset();
                    let m = [p[0] + o[0], p[1], p[2] + o[2]];
                    if replaceable_dry(cx.r, m)
                        && !Dir::NESW.iter().any(|dd| is_shelf(cx.r, add(m, *dd)))
                        && !Dir::NESW.iter().any(|dd| is_shelf(cx.r, add(p, *dd)))
                    {
                        place(cx, rng, m, d);
                    }
                }
            }
        }
    } else {
        let first = random_horizontal(rng);
        let dirs = [first, first.cw()];
        let base = logs[0][1];
        for &p in logs {
            let dy = p[1] - base;
            if !(1..=4).contains(&dy) {
                continue;
            }
            for d in dirs {
                if rng.next_float() <= 0.25 {
                    let o = d.offset();
                    let m = [p[0] + o[0], p[1], p[2] + o[2]];
                    if replaceable_dry(cx.r, m) && !is_shelf(cx.r, above(m, -1)) {
                        place(cx, rng, m, d);
                        break;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- Leaf distances

fn update_leaves(cx: &mut Ctx) {
    let s = &mut cx.tree;
    // Bounding box.
    let mut mn = [i32::MAX; 3];
    let mut mx = [i32::MIN; 3];
    for set in [&s.roots, &s.trunks, &s.foliage, &s.decorations] {
        for p in set.iter() {
            for k in 0..3 {
                mn[k] = mn[k].min(p[k]);
                mx[k] = mx[k].max(p[k]);
            }
        }
    }
    if mn[0] > mx[0] {
        return;
    }
    let size = [mx[0] - mn[0] + 1, mx[1] - mn[1] + 1, mx[2] - mn[2] + 1];
    let n = (size[0] * size[1] * size[2]) as usize;
    s.shape.clear();
    s.shape.resize(n.div_ceil(64), 0);
    let idx = |p: [i32; 3]| ((p[0] - mn[0]) * size[1] * size[2] + (p[1] - mn[1]) * size[2] + (p[2] - mn[2])) as usize;
    let inside = |p: [i32; 3]| (0..3).all(|k| p[k] >= mn[k] && p[k] <= mx[k]);
    for set in [&s.decorations, &s.roots] {
        for p in set.iter() {
            let i = idx(p);
            s.shape[i >> 6] |= 1 << (i & 63);
        }
    }
    for c in &mut s.to_check {
        c.clear();
    }
    if crate::opt::tree_take() {
        for p in s.trunks.iter() {
            s.to_check[0].insert(p);
        }
    } else {
        let trunk_iter: Vec<[i32; 3]> = s.trunks.iter().collect();
        for p in trunk_iter {
            s.to_check[0].insert(p);
        }
    }
    let b = cx.r.b;
    let mut sd = 0usize;
    loop {
        while sd >= 7 || !cx.tree.to_check[sd].is_empty() {
            if sd >= 7 {
                edge_updates(cx, mn, size);
                return;
            }
            let Some(p) = cx.tree.to_check[sd].pop_first() else { break };
            if !inside(p) {
                continue;
            }
            if sd != 0 {
                let st = cx.r.get_p(p);
                let ns = b.with_int(st, b.p.distance, sd as i32);
                cx.r.set_p(p, ns, FLAGS);
            }
            let i = idx(p);
            cx.tree.shape[i >> 6] |= 1 << (i & 63);
            for d in Dir::ALL {
                let q = add(p, d);
                if !inside(q) {
                    continue;
                }
                let qi = idx(q);
                if cx.tree.shape[qi >> 6] >> (qi & 63) & 1 != 0 {
                    continue;
                }
                let st = cx.r.get_p(q);
                let dist =
                    if b.in_tag(st, b.rules.prevents_leaf_decay) { Some(0) } else { b.get_int(st, b.p.distance) };
                if let Some(dd) = dist {
                    let nd = dd.min(sd as i32 + 1);
                    if nd < 7 {
                        cx.tree.to_check[nd as usize].insert(q);
                        sd = sd.min(nd as usize);
                    }
                }
            }
        }
        sd += 1;
    }
}

/// Shape updates on the faces of the voxel shape (edge of the tree).
fn edge_updates(cx: &mut Ctx, mn: [i32; 3], size: [i32; 3]) {
    let full = |cx: &Ctx, x: i32, y: i32, z: i32| {
        let i = (x * size[1] * size[2] + y * size[2] + z) as usize;
        cx.tree.shape[i >> 6] >> (i & 63) & 1 != 0
    };
    // Passes: (axis c, a, b) = (z; x, y), (y; z, x), (x; y, z).
    for pass in 0..3 {
        let (asz, bsz, csz) = match pass {
            0 => (size[0], size[1], size[2]),
            1 => (size[2], size[0], size[1]),
            _ => (size[1], size[2], size[0]),
        };
        let (neg, pos) = match pass {
            0 => (Dir::North, Dir::South),
            1 => (Dir::Down, Dir::Up),
            _ => (Dir::West, Dir::East),
        };
        let xyz = |a: i32, b: i32, c: i32| match pass {
            0 => (a, b, c),
            1 => (b, c, a),
            _ => (c, a, b),
        };
        for a in 0..asz {
            for bb in 0..bsz {
                let mut last = false;
                for c in 0..=csz {
                    let f = c != csz && {
                        let (x, y, z) = xyz(a, bb, c);
                        full(cx, x, y, z)
                    };
                    if !last && f {
                        let (x, y, z) = xyz(a, bb, c);
                        face_update(cx, [mn[0] + x, mn[1] + y, mn[2] + z], neg);
                    }
                    if last && !f {
                        let (x, y, z) = xyz(a, bb, c - 1);
                        face_update(cx, [mn[0] + x, mn[1] + y, mn[2] + z], pos);
                    }
                    last = f;
                }
            }
        }
    }
}

fn face_update(cx: &mut Ctx, p: [i32; 3], d: Dir) {
    let q = add(p, d);
    let s = cx.r.get_p(p);
    let n = cx.r.get_p(q);
    // Block that no shape update changes: call skipped (`RUSTCRAFT_OPT_TREE_FACESKIP`).
    let b = cx.r.b;
    let skip = crate::opt::tree_faceskip();
    let ns = if skip && b.shape.inert(b.block(s)) { s } else { crate::shape_update::update(cx.r, s, p, d, n) };
    if ns != s {
        cx.r.set_p(p, ns, 2);
    }
    if skip && b.shape.inert(b.block(n)) {
        return;
    }
    let nn = crate::shape_update::update(cx.r, n, q, d.opposite(), ns);
    if nn != n {
        cx.r.set_p(q, nn, 2);
    }
}

// ---------------------------------------------------------------- Fallen trunk

pub fn place_fallen(cx: &mut Ctx, t: &FallenTree, rng: &mut DecoRng, origin: [i32; 3]) -> bool {
    let b = cx.r.b;
    // Stump.
    let stump = t.trunk_provider.get(cx.r, rng, origin);
    cx.r.set_p(origin, stump, 3);
    cx.r.mark_above(origin);
    if !t.stump_decorators.is_empty() {
        let s = &mut cx.tree;
        s.logs.clear();
        s.logs.push(origin);
        s.leaves.clear();
        s.root_list.clear();
        for d in &t.stump_decorators {
            run_fallen_decorator(cx, d, rng);
        }
    }
    let dir = random_horizontal(rng);
    let len = t.log_length.sample(rng) - 2;
    let mut start = add_n(origin, dir, 2 + rng.next_int_bounded(2));
    start = above(start, 1);
    // Search for a height where to place the trunk.
    let mut found = false;
    for _ in 0..6 {
        if can_place_fallen_at(cx.r, start) {
            found = true;
            break;
        }
        start = above(start, -1);
    }
    let _ = found;
    // Check.
    let mut ok = true;
    let mut hole = 0;
    for i in 0..len {
        let p = add_n(start, dir, i);
        if !valid_tree_pos(cx.r, p) {
            ok = false;
            break;
        }
        if !b.sturdy(cx.r.get_p(above(p, -1)), Dir::Up) {
            hole += 1;
            if hole > 2 {
                ok = false;
                break;
            }
        } else {
            hole = 0;
        }
    }
    if ok {
        let mut placed = Vec::new();
        for i in 0..len {
            let p = add_n(start, dir, i);
            let s = t.trunk_provider.get(cx.r, rng, p);
            let s = b.with_axis(s, b.p.axis, dir.axis());
            cx.r.set_p(p, s, 3);
            cx.r.mark_above(p);
            placed.push(p);
        }
        if !t.log_decorators.is_empty() {
            let mut set = JSet::default();
            for p in &placed {
                set.insert(*p);
            }
            let s = &mut cx.tree;
            set.sorted_by_y(&mut s.logs);
            s.leaves.clear();
            s.root_list.clear();
            for d in &t.log_decorators {
                run_fallen_decorator(cx, d, rng);
            }
        }
    }
    true
}

fn can_place_fallen_at(r: &Region, p: [i32; 3]) -> bool {
    valid_tree_pos(r, p) && r.b.sturdy(r.get_p(above(p, -1)), Dir::Up)
}

/// Decorators of a fallen trunk: placements go through the "place and update" block.
fn run_fallen_decorator(cx: &mut Ctx, d: &TreeDecorator, rng: &mut DecoRng) {
    cx.tree.decorations.clear();
    run_decorator(cx, d, rng);
}

#[allow(dead_code)]
fn unused(_: f32) -> f32 {
    opt_f32(&Json::Null, "", 0.0)
}
