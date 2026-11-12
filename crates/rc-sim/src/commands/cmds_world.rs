//! World commands: `setblock`, `fill`, `clone`, `forceload`, `worldborder`.

use std::collections::BTreeSet;
use std::path::Path;

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::src::{Fail, Src};
use super::text::{self, f2, lit};
use super::{Parsed, R, Seg, args};
use crate::blocks::flag;
use crate::server::Server;
use crate::tr;

fn tickets_path(dim_dir: &Path) -> std::path::PathBuf {
    dim_dir.join("data").join("minecraft").join("chunk_tickets.dat")
}

/// Force-loaded chunks: `minecraft:forced` tickets of `chunk_tickets.dat` (dimension).
pub fn read_forced(dim_dir: &Path) -> BTreeSet<(i32, i32)> {
    let mut out = BTreeSet::new();
    let Ok(root) = crate::nbtio::read_dat(&tickets_path(dim_dir)) else { return out };
    for t in crate::nbtio::list(root.get("data").and_then(|d| d.get("tickets"))) {
        if t.get("type").and_then(Nbt::as_str) != Some("minecraft:forced") {
            continue;
        }
        if let Some(Nbt::IntArray(a)) = t.get("chunk_pos")
            && a.len() == 2
        {
            out.insert((a[0], a[1]));
        }
    }
    out
}

/// `chunk_tickets.dat` rewritten: other tickets kept, forced tickets replaced.
pub(crate) fn forced_job(dim_dir: &Path, set: &BTreeSet<(i32, i32)>) -> crate::save::Job {
    let path = tickets_path(dim_dir);
    let old = crate::nbtio::read_dat(&path).ok();
    let mut tickets: Vec<Nbt> =
        crate::nbtio::list(old.as_ref().and_then(|r| r.get("data")).and_then(|d| d.get("tickets")))
            .iter()
            .filter(|t| t.get("type").and_then(Nbt::as_str) != Some("minecraft:forced"))
            .cloned()
            .collect();
    for &(x, z) in set {
        tickets.push(Nbt::Compound(vec![
            ("chunk_pos".into(), Nbt::IntArray(vec![x, z])),
            ("type".into(), Nbt::String("minecraft:forced".into())),
            ("level".into(), Nbt::Int(31)),
        ]));
    }
    let root = Nbt::Compound(vec![
        ("data".into(), Nbt::Compound(vec![("tickets".into(), Nbt::List(tickets))])),
        ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
    ]);
    crate::save::Job::Dat { path, nbt: crate::nbtio::encode_root(&root), backup: false }
}

fn bbox(a: [i32; 3], b: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    ([a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])], [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])])
}

/// One block copied by `clone`: where it goes, its state, its block entity data (the live one),
/// and the state found at the destination before the copy (for the final neighbour updates).
struct ClonedBlock {
    to: [i32; 3],
    state: u32,
    data: Option<Nbt>,
    replaced: u32,
}

/// The source box of `clone`, sorted like the game: solid blocks (opaque or full collision),
/// blocks with a block entity, the others; and the order a move clears the source in (the
/// others first).
#[derive(Default)]
struct CloneSource {
    solid: Vec<ClonedBlock>,
    with_block_entity: Vec<ClonedBlock>,
    others: Vec<ClonedBlock>,
    clear_order: std::collections::VecDeque<[i32; 3]>,
}

/// Mode of `fill`, read once from the command's path.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FillMode {
    Replace,
    Keep,
    Outline,
    Hollow,
    Destroy,
    Strict,
}

impl FillMode {
    fn of(path: &[String]) -> FillMode {
        let word = path.iter().skip(4).find_map(|w| match w.as_str() {
            "outline" => Some(FillMode::Outline),
            "hollow" => Some(FillMode::Hollow),
            "destroy" => Some(FillMode::Destroy),
            "keep" => Some(FillMode::Keep),
            "strict" => Some(FillMode::Strict),
            _ => None,
        });
        word.unwrap_or(FillMode::Replace)
    }
}

/// The block filter of `fill … replace <filter>` and `clone … filtered`: read once for the
/// command (`RUSTCRAFT_OPT_FILLPREDICATE`), or its text read again at each block (former path).
enum BlockFilter {
    Text(String),
    Pred(Box<args::BlockPred>),
}

impl Server {
    fn block_filter(&self, text: Option<String>) -> Option<BlockFilter> {
        let text = text?;
        Some(if self.cmd.paths.fill_predicate {
            BlockFilter::Pred(Box::new(args::BlockPred::new(&text)))
        } else {
            BlockFilter::Text(text)
        })
    }

    fn filter_matches(&self, filter: &mut BlockFilter, p: [i32; 3]) -> bool {
        match filter {
            BlockFilter::Text(text) => self.block_matches(text, p),
            BlockFilter::Pred(pred) => self.block_pred_matches(pred, p),
        }
    }
}

impl Server {
    fn max_modifications(&self) -> i64 {
        self.rules.get("minecraft:max_block_modifications").and_then(Nbt::as_i64).unwrap_or(32768)
    }

    /// Updated shape (non-strict), given properties kept, then
    /// block entity data loaded from the given NBT ALONE (absent fields take
    /// their default value, no old value kept); counts as modified if the
    /// data changed.
    fn place_input(&mut self, p: [i32; 3], b: &args::BlockInput, strict: bool) -> bool {
        let mut st = if strict { b.state } else { self.update_from_neighbour_shapes(b.state, p) };
        let v = self.view();
        if v.is_air(st) {
            st = b.state;
        }
        for (k, val) in &b.props {
            st = v.set_named(st, k, val);
        }
        let flags = flag::CLIENTS | if strict { 816 } else { flag::SKIP_BE_SIDE_EFFECTS };
        let mut affected = self.set_block(p, st, flags);
        if let Some(n) = &b.nbt
            && let Some(before) = self.live_block_data(p)
        {
            self.load_block_data(p, n.clone());
            if !self.live_block_data(p).is_some_and(|after| crate::items::nbt_eq(&after, &before)) {
                affected = true;
                self.send_block_entity(p);
            }
        }
        affected
    }

    /// `place_input` of one `fill` block; the former path placed a copy of the input (its data
    /// and properties) at every block.
    fn fill_place(&mut self, pos: [i32; 3], input: &args::BlockInput, strict: bool) -> bool {
        if self.cmd.paths.fill_predicate {
            return self.place_input(pos, input, strict);
        }
        let copy = args::BlockInput { state: input.state, nbt: input.nbt.clone(), props: input.props.clone() };
        self.place_input(pos, &copy, strict)
    }

    /// The neighbour update on placement.
    fn neighbours_on_set(&mut self, p: [i32; 3], old: u32) {
        let v = self.view();
        let now = self.get(p);
        let nb = v.block(now);
        if v.block(old) != nb {
            self.affect_after_removal(old, p, false);
        }
        self.update_neighbors_at(p, nb);
        if self.view().has_analog(now) {
            self.update_neighbour_for_output_signal(p, nb);
        }
    }

    pub(crate) fn cmd_setblock(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let pos = self.loaded_pos(s, &p.arg(seg, "pos").unwrap_or_default())?;
        let Some(b) = self.block_input(&p.arg(seg, "block").unwrap_or_default()) else {
            return Err(Fail(tr!("commands.setblock.failed")));
        };
        let mode = seg.path.get(3).map(String::as_str).unwrap_or("replace");
        let strict = mode == "strict";
        if mode == "keep" && !self.view().is_air(self.get(pos)) {
            return Err(Fail(tr!("commands.setblock.failed")));
        }
        let place = if mode == "destroy" {
            self.destroy_block(pos, true, 512);
            !self.view().is_air(b.state) || !self.view().is_air(self.get(pos))
        } else {
            true
        };
        let old = self.get(pos);
        if place && !self.place_input(pos, &b, strict) {
            return Err(Fail(tr!("commands.setblock.failed")));
        }
        if !strict {
            self.neighbours_on_set(pos, old);
        }
        self.ok_with(s, |_| tr!("commands.setblock.success", pos[0], pos[1], pos[2]));
        Ok(1)
    }

    pub(crate) fn cmd_fill(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let from = self.loaded_pos(s, &p.arg(seg, "from").unwrap_or_default())?;
        let to = self.loaded_pos(s, &p.arg(seg, "to").unwrap_or_default())?;
        let (min, max) = bbox(from, to);
        let area: i64 = (0..3).map(|k| i64::from(max[k] - min[k] + 1)).product();
        let limit = self.max_modifications();
        if area > limit {
            return Err(Fail(tr!("commands.fill.toobig", limit, area)));
        }
        let Some(b) = self.block_input(&p.arg(seg, "block").unwrap_or_default()) else {
            return Err(Fail(tr!("commands.fill.failed")));
        };
        let mode = FillMode::of(&seg.path);
        let mut filter = self.block_filter(p.arg(seg, "filter"));
        let strict = mode == FillMode::Strict;
        let air = self.block_input("minecraft:air");
        let mut updated = Vec::new();
        let mut count = 0;
        for z in min[2]..=max[2] {
            for y in min[1]..=max[1] {
                for x in min[0]..=max[0] {
                    let pos = [x, y, z];
                    if mode == FillMode::Keep && !self.view().is_air(self.get(pos)) {
                        continue;
                    }
                    if let Some(f) = filter.as_mut()
                        && !self.filter_matches(f, pos)
                    {
                        continue;
                    }
                    let old = self.get(pos);
                    let affected = mode == FillMode::Destroy && self.destroy_block(pos, true, 512);
                    let edge = x == min[0] || x == max[0] || y == min[1] || y == max[1] || z == min[2] || z == max[2];
                    let input = match mode {
                        FillMode::Outline if !edge => None,
                        FillMode::Hollow if !edge => air.as_ref(),
                        _ => Some(&b),
                    };
                    let placed = input.is_some_and(|i| self.fill_place(pos, i, strict));
                    if placed {
                        if !strict {
                            updated.push((pos, old));
                        }
                        count += 1;
                    } else if affected {
                        count += 1;
                    }
                }
            }
        }
        for (pos, old) in updated {
            self.neighbours_on_set(pos, old);
        }
        if count == 0 {
            return Err(Fail(tr!("commands.fill.failed")));
        }
        self.ok_with(s, |_| tr!("commands.fill.success", count));
        Ok(count)
    }

    pub(crate) fn cmd_clone(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let start = args::block_pos(self, s, &a("begin"));
        let end = args::block_pos(self, s, &a("end"));
        let dest = args::block_pos(self, s, &a("destination"));
        // Source dimension: the one named after `from`, otherwise the source's; target: the one
        // named after `to`, otherwise the source dimension.
        let here = self.dim.name.clone();
        let src_dim = p.arg(seg, "sourceDimension").map_or_else(|| here.clone(), |d| args::id(&d));
        let dst_dim = p.arg(seg, "targetDimension").map_or_else(|| src_dim.clone(), |d| args::id(&d));
        for d in [&src_dim, &dst_dim] {
            if *d != here && !self.world_dimensions().contains(d) {
                return Err(Fail(tr!("argument.dimension.invalid", d.clone())));
            }
        }
        let (min, max) = bbox(start, end);
        let len = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
        let dmax = [dest[0] + len[0], dest[1] + len[1], dest[2] + len[2]];
        let mode = w.iter().rev().find(|x| matches!(**x, "force" | "move" | "normal")).copied().unwrap_or("normal");
        let mask = w.iter().find(|x| matches!(**x, "replace" | "masked" | "filtered")).copied().unwrap_or("replace");
        let strict = w.contains(&"strict");
        let overlap = (0..3).all(|k| dest[k] <= max[k] && dmax[k] >= min[k]);
        if mode == "normal" && src_dim == dst_dim && overlap {
            return Err(Fail(tr!("commands.clone.overlap")));
        }
        let area: i64 = (0..3).map(|k| i64::from(len[k] + 1)).product();
        let limit = self.max_modifications();
        if area > limit {
            return Err(Fail(tr!("commands.clone.toobig", limit, area)));
        }
        let loaded = |me: &Server, a: [i32; 3], b: [i32; 3]| {
            (a[0] >> 4..=b[0] >> 4).all(|cx| (a[2] >> 4..=b[2] >> 4).all(|cz| me.is_loaded([cx << 4, 0, cz << 4])))
        };
        if !self.in_world(&src_dim, |me| loaded(me, min, max)) || !self.in_world(&dst_dim, |me| loaded(me, dest, dmax))
        {
            return Err(Fail(tr!("argument.pos.unloaded")));
        }
        let mut filter = self.block_filter(p.arg(seg, "filter"));
        let source = self.in_world(&src_dim, |me| me.clone_scan(min, max, dest, mask, filter.as_mut()));
        let base = flag::CLIENTS | if strict { 816 } else { 0 };
        let barrier = self.view().default_state(self.table().block_id("barrier"));
        let air = self.air();
        let CloneSource { solid, with_block_entity, others, clear_order } = source;
        let mut all: Vec<ClonedBlock> = Vec::new();
        all.extend(solid);
        let n_solid = all.len();
        all.extend(with_block_entity);
        let n_with_data = all.len() - n_solid;
        all.extend(others);
        // State found at the destination during the scan, before a move clears the source
        // (removed state of the final neighbour pass, overlapping boxes included).
        self.in_world(&dst_dim, |me| {
            for block in &mut all {
                block.replaced = me.get(block.to);
            }
        });
        if mode == "move" {
            self.in_world(&src_dim, |me| {
                for &pos in &clear_order {
                    me.set_block(pos, barrier, base | 816);
                }
                let std_flags = if strict { base } else { flag::ALL };
                for &pos in &clear_order {
                    me.set_block(pos, air, std_flags);
                }
            });
        }
        let with_data = n_solid..n_solid + n_with_data;
        let count = self.in_world(&dst_dim, |me| me.clone_place(&all, with_data, barrier, base, strict));
        // Scheduled block ticks of the source box (fluids are not copied), moved by the
        // offset with the same due time and priority, ordered after the copied ones.
        let ticks = self.in_world(&src_dim, |me| me.bs.block_ticks.in_box(min, max));
        if !ticks.is_empty() {
            let off = [dest[0] - min[0], dest[1] - min[1], dest[2] - min[2]];
            let lo = ticks.iter().map(|t| t.sub).min().unwrap_or(0);
            let hi = ticks.iter().map(|t| t.sub).max().unwrap_or(0);
            self.in_world(&dst_dim, |me| {
                for t in &ticks {
                    let q = [t.pos[0] + off[0], t.pos[1] + off[1], t.pos[2] + off[2]];
                    if me.bs.block_ticks.schedule(q, t.kind, t.at, t.priority, t.sub - lo + hi + 1) {
                        me.mark_dirty(q);
                    }
                }
            });
        }
        if count == 0 {
            return Err(Fail(tr!("commands.clone.failed")));
        }
        self.ok_with(s, |_| tr!("commands.clone.success", count));
        Ok(count)
    }

    /// Source side of `clone`: the blocks the mask keeps, sorted like the game.
    fn clone_scan(
        &self,
        min: [i32; 3],
        max: [i32; 3],
        dest: [i32; 3],
        mask: &str,
        mut filter: Option<&mut BlockFilter>,
    ) -> CloneSource {
        let air_block = self.table().block_id("air");
        let mut out = CloneSource::default();
        for z in min[2]..=max[2] {
            for y in min[1]..=max[1] {
                for x in min[0]..=max[0] {
                    let from = [x, y, z];
                    let to = [dest[0] + x - min[0], dest[1] + y - min[1], dest[2] + z - min[2]];
                    let state = self.get(from);
                    let kept = match mask {
                        "masked" => self.view().block(state) != air_block,
                        "filtered" => filter.as_deref_mut().is_some_and(|f| self.filter_matches(f, from)),
                        _ => true,
                    };
                    if !kept {
                        continue;
                    }
                    let info = self.view().info(state);
                    // The live data: a furnace's NBT lags until the save.
                    if let Some(data) = self.live_block_data(from) {
                        out.with_block_entity.push(ClonedBlock { to, state, data: Some(data), replaced: 0 });
                        out.clear_order.push_back(from);
                    } else if !info.has(rc_data::flags::SOLID_RENDER) && !info.has(rc_data::flags::FULL_COLLISION) {
                        out.others.push(ClonedBlock { to, state, data: None, replaced: 0 });
                        out.clear_order.push_front(from);
                    } else {
                        out.solid.push(ClonedBlock { to, state, data: None, replaced: 0 });
                        out.clear_order.push_back(from);
                    }
                }
            }
        }
        out
    }

    /// Destination side of `clone`: barriers first (in reverse), then every state, then the
    /// block entity data of `with_data`, then the neighbour updates; the number of blocks set.
    fn clone_place(
        &mut self,
        all: &[ClonedBlock],
        with_data: std::ops::Range<usize>,
        barrier: u32,
        base: u32,
        strict: bool,
    ) -> i32 {
        for block in all.iter().rev() {
            self.set_block(block.to, barrier, base | 816);
        }
        let mut count = 0;
        for block in all {
            if self.set_block(block.to, block.state, base) {
                count += 1;
            }
        }
        for block in &all[with_data] {
            if let Some(mut n) = block.data.clone()
                && self.be_get(block.to).is_some()
            {
                for k in ["x", "y", "z"] {
                    crate::nbtio::remove(&mut n, k);
                }
                self.set_block_data(block.to, n);
            }
            self.set_block(block.to, block.state, base);
        }
        if !strict {
            for block in all.iter().rev() {
                self.neighbours_on_set(block.to, block.replaced);
            }
        }
        count
    }

    /// Loads what forced chunks keep loaded, like the game's forced ticket: each forced chunk and
    /// every chunk within two of it, read or generated, so that it and its neighbours tick.
    pub(crate) fn load_forced_chunks(&mut self, forced: &[(i32, i32)]) {
        let mut wanted: Vec<(i32, i32)> = forced
            .iter()
            .flat_map(|&(x, z)| (-2..=2).flat_map(move |dx| (-2..=2).map(move |dz| (x + dx, z + dz))))
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        self.ensure_loaded(&wanted);
    }

    /// Opening: every other world whose saved forced chunks are not empty is opened and those
    /// chunks loaded, like the game which loads each world's forced chunks before the player
    /// enters; they tick from the first tick (`dims_tick`). The host's own world is left as before
    /// (see docs/specs/integrated-server.md, section 10).
    pub(crate) fn open_other_worlds_with_forced_chunks(&mut self) {
        for name in self.world_dimensions() {
            if name == self.dim.name {
                continue;
            }
            let has_forced = match self.dims.parked.iter().find(|w| w.dim.name == name) {
                Some(w) => !w.forced.is_empty(),
                None => !read_forced(&crate::world::dimension_dir(&self.world, &name)).is_empty(),
            };
            if has_forced {
                self.in_world(&name, |s| {
                    let forced: Vec<(i32, i32)> = s.cmd.forced.iter().copied().collect();
                    s.load_forced_chunks(&forced);
                });
            }
        }
    }

    pub(crate) fn cmd_forceload(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let dim = s.dim.clone();
        let chunk_text = |c: (i32, i32)| format!("[{}, {}]", c.0, c.1);
        match w.get(1).copied() {
            Some("query") => match p.arg(seg, "pos") {
                Some(t) => {
                    let [x, z] = args::column(s, &t);
                    let c = (x >> 4, z >> 4);
                    if self.cmd.forced.contains(&c) {
                        self.ok_with(s, |_| tr!("commands.forceload.query.success", chunk_text(c), dim));
                        Ok(1)
                    } else {
                        Err(Fail(tr!("commands.forceload.query.failure", chunk_text(c), dim)))
                    }
                }
                None => {
                    let n = self.cmd.forced.len();
                    if n == 0 {
                        self.fail(s, tr!("commands.forceload.added.none", dim));
                    } else {
                        // Listed in the order of the packed position (z then x as an unsigned
                        // 32-bit value), like the game.
                        let mut sorted: Vec<(i32, i32)> = self.cmd.forced.iter().copied().collect();
                        sorted.sort_unstable_by_key(|c| (i64::from(c.1) << 32) | i64::from(c.0 as u32));
                        let list = sorted.into_iter().map(chunk_text).collect::<Vec<_>>().join(", ");
                        if n == 1 {
                            self.ok_with(s, |_| tr!("commands.forceload.list.single", dim, list));
                        } else {
                            self.ok_with(s, |_| tr!("commands.forceload.list.multiple", n, dim, list));
                        }
                    }
                    Ok(n as i32)
                }
            },
            Some("remove") if w.get(2) == Some(&"all") => {
                self.cmd.forced.clear();
                self.cmd.forced_dirty = true;
                self.ok_with(s, |_| tr!("commands.forceload.removed.all", dim));
                Ok(0)
            }
            Some(op @ ("add" | "remove")) => {
                let add = op == "add";
                let [x1, z1] = args::column(s, &p.arg(seg, "from").unwrap_or_default());
                let [x2, z2] = p.arg(seg, "to").map_or([x1, z1], |t| args::column(s, &t));
                let (bx0, bz0, bx1, bz1) = (x1.min(x2), z1.min(z2), x1.max(x2), z1.max(z2));
                if bx0 < -30_000_000 || bz0 < -30_000_000 || bx1 >= 30_000_000 || bz1 >= 30_000_000 {
                    return Err(Fail(tr!("argument.pos.outofworld")));
                }
                let (cx0, cz0, cx1, cz1) = (bx0 >> 4, bz0 >> 4, bx1 >> 4, bz1 >> 4);
                let n = i64::from(cx1 - cx0 + 1) * i64::from(cz1 - cz0 + 1);
                if n > 256 {
                    return Err(Fail(tr!("commands.forceload.toobig", 256, n)));
                }
                let mut changed = Vec::new();
                for cx in cx0..=cx1 {
                    for cz in cz0..=cz1 {
                        let did =
                            if add { self.cmd.forced.insert((cx, cz)) } else { self.cmd.forced.remove(&(cx, cz)) };
                        if did {
                            changed.push((cx, cz));
                        }
                    }
                }
                if changed.is_empty() {
                    return Err(Fail(tr!(if add {
                        "commands.forceload.added.failure"
                    } else {
                        "commands.forceload.removed.failure"
                    })));
                }
                self.cmd.forced_dirty = true;
                if add {
                    self.load_forced_chunks(&changed);
                }
                let k = if add { "added" } else { "removed" };
                if changed.len() == 1 {
                    self.ok_with(s, |_| tr!(&format!("commands.forceload.{k}.single"), chunk_text(changed[0]), dim));
                } else {
                    let (a0, b0) = (chunk_text((cx0, cz0)), chunk_text((cx1, cz1)));
                    self.ok_with(s, |_| tr!(&format!("commands.forceload.{k}.multiple"), changed.len(), dim, a0, b0));
                }
                Ok(changed.len() as i32)
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    pub(crate) fn cmd_worldborder(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        let b = &mut self.cmd.border;
        let r = match w.get(1).copied() {
            Some("get") => {
                let size = b.size;
                self.ok_with(s, |_| tr!("commands.worldborder.get", format!("{size:.0}")));
                return Ok((size + 0.5).floor() as i32);
            }
            Some("center") => {
                let [x, z] = args::vec2(s, &a("pos"));
                if b.center == [x, z] {
                    return Err(Fail(tr!("commands.worldborder.center.failed")));
                }
                if x.abs() > 2.9999984e7 || z.abs() > 2.9999984e7 {
                    return Err(Fail(tr!("commands.worldborder.set.failed.far", "2.9999984E7")));
                }
                b.center = [x, z];
                let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::Center { x, z }));
                self.ok_with(s, |_| tr!("commands.worldborder.center.success", f2(x), f2(z)));
                0
            }
            Some("damage") => {
                let v: f32 =
                    a(if w.get(2) == Some(&"amount") { "damagePerBlock" } else { "distance" }).parse().unwrap_or(0.0);
                if w.get(2) == Some(&"amount") {
                    if b.damage == v {
                        return Err(Fail(tr!("commands.worldborder.damage.amount.failed")));
                    }
                    b.damage = v;
                    self.ok_with(s, |_| tr!("commands.worldborder.damage.amount.success", format!("{v:.2}")));
                } else {
                    if b.buffer == v {
                        return Err(Fail(tr!("commands.worldborder.damage.buffer.failed")));
                    }
                    b.buffer = v;
                    self.ok_with(s, |_| tr!("commands.worldborder.damage.buffer.success", format!("{v:.2}")));
                }
                v as i32
            }
            Some("warning") => {
                if w.get(2) == Some(&"time") {
                    let t = super::cmds_basic::time_ticks(&a("time"));
                    if b.warn_time == t {
                        return Err(Fail(tr!("commands.worldborder.warning.time.failed")));
                    }
                    b.warn_time = t;
                    let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::WarningDelay(t)));
                    self.ok_with(s, |_| tr!("commands.worldborder.warning.time.success", secs(t)));
                    t
                } else {
                    let d: i32 = a("distance").parse().unwrap_or(0);
                    if b.warn_blocks == d {
                        return Err(Fail(tr!("commands.worldborder.warning.distance.failed")));
                    }
                    b.warn_blocks = d;
                    let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::WarningDistance(d)));
                    self.ok_with(s, |_| tr!("commands.worldborder.warning.distance.success", d));
                    d
                }
            }
            Some(op @ ("set" | "add")) => {
                let d: f64 = a("distance").parse().unwrap_or(0.0);
                let mut t = p.arg(seg, "time").map_or(0, |x| super::cmds_basic::time_ticks(&x));
                // `add` with a duration extends the transition in progress: its remaining
                // ticks are added to the given time.
                if op == "add" && p.arg(seg, "time").is_some() {
                    t = t.saturating_add(b.lerp.map_or(0, |l| l.3.clamp(0, i64::from(i32::MAX)) as i32));
                }
                let target = if op == "add" { b.size + d } else { d };
                if target == b.size {
                    return Err(Fail(tr!("commands.worldborder.set.failed.nochange")));
                }
                if target < 1.0 {
                    return Err(Fail(tr!("commands.worldborder.set.failed.small")));
                }
                if target > 5.999_996_8e7 {
                    return Err(Fail(tr!("commands.worldborder.set.failed.big", "5.999997E7")));
                }
                let old = b.size;
                // Transition: the size starts from the current one and advances one step per world tick.
                if t > 0 {
                    b.lerp = Some((old, target, i64::from(t), i64::from(t)));
                } else {
                    b.lerp = None;
                    b.size = target;
                }
                let dist = format!("{target:.1}");
                if t > 0 {
                    let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::Lerp {
                        old_size: old,
                        new_size: target,
                        ticks: i64::from(t),
                    }));
                    let k =
                        if target > old { "commands.worldborder.set.grow" } else { "commands.worldborder.set.shrink" };
                    self.ok_with(s, |_| tr!(k, dist, secs(t)));
                } else {
                    let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::Size(target)));
                    self.ok_with(s, |_| tr!("commands.worldborder.set.immediate", dist));
                }
                (target - old) as i32
            }
            _ => return Err(Fail(tr!("command.unknown.command"))),
        };
        self.cmd.border_dirty = true;
        Ok(r)
    }
}

fn secs(t: i32) -> String {
    format!("{:.2}", f64::from(t) / 20.0)
}

impl Server {
    /// Border tick (before weather, if the game is running): the border of every dimension moves
    /// one step, the active one and the others, like every world's border in the game.
    pub(crate) fn border_tick(&mut self) {
        if self.cmd.border.tick() {
            self.cmd.border_dirty = true;
        }
        for other in &mut self.cmd.other_borders {
            if other.border.tick() {
                other.unsaved = true;
            }
        }
    }

    /// The active dimension changes to `entering`: its border becomes the active one, the active
    /// one is kept under the name of the dimension left.
    pub(crate) fn swap_active_border(&mut self, entering: &str) {
        let leaving = &self.dim.name;
        let cmd = &mut self.cmd;
        let i = match cmd.other_borders.iter().position(|b| b.dimension == entering) {
            Some(i) => i,
            // Safety net: every dimension of the world settings is read at opening; one missing
            // from them still gets its saved border, read now.
            None => {
                let border = super::Border::read(&crate::world::dimension_dir(&self.world, entering));
                cmd.other_borders.push(super::DimensionBorder {
                    dimension: entering.to_owned(),
                    border,
                    unsaved: false,
                });
                cmd.other_borders.len() - 1
            }
        };
        let kept = &mut cmd.other_borders[i];
        std::mem::swap(&mut cmd.border, &mut kept.border);
        std::mem::swap(&mut cmd.border_dirty, &mut kept.unsaved);
        // The name's buffer is reused: every visit of a parked world swaps the borders twice.
        kept.dimension.clear();
        kept.dimension.push_str(leaving);
    }

    /// Border damage to the current player (excluding suffocation): box
    /// outside the limits, distance to the edge + negative safe zone → max(1, ⌊−d × damage⌋).
    pub(crate) fn border_damage(&self) -> Option<f32> {
        let b = &self.cmd.border;
        let half = b.size / 2.0;
        let (min_x, max_x) = ((b.center[0] - half).max(-29_999_984.0), (b.center[0] + half).min(29_999_984.0));
        let (min_z, max_z) = ((b.center[1] - half).max(-29_999_984.0), (b.center[1] + half).min(29_999_984.0));
        let inside = |x: f64, z: f64| x >= min_x && x < max_x && z >= min_z && z < max_z;
        let bb = self.player_aabb();
        let eps = f64::from(1.0e-5f32);
        if inside(bb.min[0], bb.min[2]) && inside(bb.max[0] - eps, bb.max[2] - eps) {
            return None;
        }
        let (x, z) = (self.pos[0], self.pos[2]);
        let dist = (x - min_x).min(max_x - x).min(z - min_z).min(max_z - z) + f64::from(b.buffer);
        if dist >= 0.0 || b.damage <= 0.0 {
            return None;
        }
        Some(((-dist * f64::from(b.damage)).floor() as i32).max(1) as f32)
    }
}

#[allow(dead_code)]
fn unused() -> rc_text::Component {
    text::join(vec![lit("")])
}
