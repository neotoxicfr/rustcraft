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

impl Server {
    fn max_modifications(&self) -> i64 {
        self.rules.get("minecraft:max_block_modifications").and_then(Nbt::as_i64).unwrap_or(32768)
    }

    /// Updated shape (non-strict), given properties kept, then
    /// block entity data loaded from the given NBT ALONE (absent fields take
    /// their default value, no old value kept); counts as modified if the
    /// data changed.
    fn place_input(&mut self, p: [i32; 3], b: &args::BlockInput, strict: bool) -> bool {
        let v = self.v();
        let mut st = if strict { b.state } else { self.update_from_neighbour_shapes(b.state, p) };
        if v.is_air(st) {
            st = b.state;
        }
        for (k, val) in &b.props {
            st = v.set(st, k, val);
        }
        let flags = flag::CLIENTS | if strict { 816 } else { flag::SKIP_BE_SIDE_EFFECTS };
        let mut affected = self.set_block(p, st, flags);
        if let Some(n) = &b.nbt
            && let Some(before) = self.be_get(p).cloned()
        {
            self.load_block_data(p, n.clone());
            if !self.be_get(p).is_some_and(|after| crate::items::nbt_eq(after, &before)) {
                affected = true;
                self.send_block_entity(p);
            }
        }
        affected
    }

    /// The neighbour update on placement.
    fn neighbours_on_set(&mut self, p: [i32; 3], old: u32) {
        let v = self.v();
        let now = self.get(p);
        let nb = v.block(now);
        if v.block(old) != nb {
            self.affect_after_removal(old, p, false);
        }
        self.update_neighbors_at(p, nb);
        if self.v().has_analog(now) {
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
        if mode == "keep" && !self.v().is_air(self.get(pos)) {
            return Err(Fail(tr!("commands.setblock.failed")));
        }
        let place = if mode == "destroy" {
            self.destroy_block(pos, true, 512);
            !self.v().is_air(b.state) || !self.v().is_air(self.get(pos))
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
        self.ok(s, tr!("commands.setblock.success", pos[0], pos[1], pos[2]));
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
        let w: Vec<&str> = seg.path.iter().map(String::as_str).collect();
        let mode =
            w.iter().skip(4).find(|x| matches!(**x, "outline" | "hollow" | "destroy" | "keep" | "strict")).copied();
        let filter = p.arg(seg, "filter");
        let strict = mode == Some("strict");
        let air = self.block_input("minecraft:air");
        let mut updated = Vec::new();
        let mut count = 0;
        for z in min[2]..=max[2] {
            for y in min[1]..=max[1] {
                for x in min[0]..=max[0] {
                    let pos = [x, y, z];
                    if mode == Some("keep") && !self.v().is_air(self.get(pos)) {
                        continue;
                    }
                    if let Some(f) = &filter
                        && !self.block_matches(f, pos)
                    {
                        continue;
                    }
                    let old = self.get(pos);
                    let mut affected = false;
                    if mode == Some("destroy") && self.destroy_block(pos, true, 512) {
                        affected = true;
                    }
                    let edge = x == min[0] || x == max[0] || y == min[1] || y == max[1] || z == min[2] || z == max[2];
                    let input = match mode {
                        Some("outline") if !edge => None,
                        Some("hollow") if !edge => air.as_ref(),
                        _ => Some(&b),
                    };
                    let placed = match input {
                        Some(i) => {
                            let i = args::BlockInput { state: i.state, nbt: i.nbt.clone(), props: i.props.clone() };
                            self.place_input(pos, &i, strict)
                        }
                        None => false,
                    };
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
        self.ok(s, tr!("commands.fill.success", count));
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
        let filter = p.arg(seg, "filter");
        type Info = ([i32; 3], u32, Option<Nbt>, u32);
        type Lists = (Vec<Info>, Vec<Info>, Vec<Info>, std::collections::VecDeque<[i32; 3]>);
        // Source side: blocks sorted like the game (solid: opaque or full collision; block
        // entities; others), clear order of a move (others first).
        let (solid, bes, other, clear): Lists = self.in_world(&src_dim, |me| {
            let air_block = me.tb().block_id("air");
            let mut l: Lists = Default::default();
            for z in min[2]..=max[2] {
                for y in min[1]..=max[1] {
                    for x in min[0]..=max[0] {
                        let sp = [x, y, z];
                        let dp = [dest[0] + x - min[0], dest[1] + y - min[1], dest[2] + z - min[2]];
                        let st = me.get(sp);
                        let ok = match mask {
                            "masked" => me.v().block(st) != air_block,
                            "filtered" => filter.as_ref().is_some_and(|f| me.block_matches(f, sp)),
                            _ => true,
                        };
                        if !ok {
                            continue;
                        }
                        let info = me.v().info(st);
                        if let Some(be) = me.be_get(sp).cloned() {
                            l.1.push((dp, st, Some(be), 0));
                            l.3.push_back(sp);
                        } else if !info.has(rc_data::flags::SOLID_RENDER) && !info.has(rc_data::flags::FULL_COLLISION) {
                            l.2.push((dp, st, None, 0));
                            l.3.push_front(sp);
                        } else {
                            l.0.push((dp, st, None, 0));
                            l.3.push_back(sp);
                        }
                    }
                }
            }
            l
        });
        let base = flag::CLIENTS | if strict { 816 } else { 0 };
        let barrier = self.v().default_state(self.tb().block_id("barrier"));
        let air = self.air();
        if mode == "move" {
            self.in_world(&src_dim, |me| {
                for &pos in &clear {
                    me.set_block(pos, barrier, base | 816);
                }
                let std_flags = if strict { base } else { flag::ALL };
                for &pos in &clear {
                    me.set_block(pos, air, std_flags);
                }
            });
        }
        let mut all: Vec<Info> = Vec::new();
        all.extend(solid);
        let n_solid = all.len();
        all.extend(bes);
        let n_bes = all.len() - n_solid;
        all.extend(other);
        let count = self.in_world(&dst_dim, |me| {
            // State found at the destination before the copy (neighbour updates at the end).
            for info in &mut all {
                info.3 = me.get(info.0);
            }
            for info in all.iter().rev() {
                me.set_block(info.0, barrier, base | 816);
            }
            let mut count = 0;
            for info in &all {
                if me.set_block(info.0, info.1, base) {
                    count += 1;
                }
            }
            for info in &all[n_solid..n_solid + n_bes] {
                if let Some(mut n) = info.2.clone()
                    && me.be_get(info.0).is_some()
                {
                    for k in ["x", "y", "z"] {
                        crate::nbtio::remove(&mut n, k);
                    }
                    me.set_block_data(info.0, n);
                }
                me.set_block(info.0, info.1, base);
            }
            if !strict {
                for info in all.iter().rev() {
                    me.neighbours_on_set(info.0, info.3);
                }
            }
            count
        });
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
        self.ok(s, tr!("commands.clone.success", count));
        Ok(count)
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
                        self.ok(s, tr!("commands.forceload.query.success", chunk_text(c), dim));
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
                            self.ok(s, tr!("commands.forceload.list.single", dim, list));
                        } else {
                            self.ok(s, tr!("commands.forceload.list.multiple", n, dim, list));
                        }
                    }
                    Ok(n as i32)
                }
            },
            Some("remove") if w.get(2) == Some(&"all") => {
                self.cmd.forced.clear();
                self.cmd.forced_dirty = true;
                self.ok(s, tr!("commands.forceload.removed.all", dim));
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
                    self.ensure_loaded(&changed);
                }
                let k = if add { "added" } else { "removed" };
                if changed.len() == 1 {
                    self.ok(s, tr!(&format!("commands.forceload.{k}.single"), chunk_text(changed[0]), dim));
                } else {
                    let (a0, b0) = (chunk_text((cx0, cz0)), chunk_text((cx1, cz1)));
                    self.ok(s, tr!(&format!("commands.forceload.{k}.multiple"), changed.len(), dim, a0, b0));
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
                self.ok(s, tr!("commands.worldborder.get", format!("{size:.0}")));
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
                self.ok(s, tr!("commands.worldborder.center.success", f2(x), f2(z)));
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
                    self.ok(s, tr!("commands.worldborder.damage.amount.success", format!("{v:.2}")));
                } else {
                    if b.buffer == v {
                        return Err(Fail(tr!("commands.worldborder.damage.buffer.failed")));
                    }
                    b.buffer = v;
                    self.ok(s, tr!("commands.worldborder.damage.buffer.success", format!("{v:.2}")));
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
                    self.ok(s, tr!("commands.worldborder.warning.time.success", secs(t)));
                    t
                } else {
                    let d: i32 = a("distance").parse().unwrap_or(0);
                    if b.warn_blocks == d {
                        return Err(Fail(tr!("commands.worldborder.warning.distance.failed")));
                    }
                    b.warn_blocks = d;
                    let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::WarningDistance(d)));
                    self.ok(s, tr!("commands.worldborder.warning.distance.success", d));
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
                    self.ok(s, tr!(k, dist, secs(t)));
                } else {
                    let _ = self.send(Event::Border(rc_proto_v777::border::BorderPacket::Size(target)));
                    self.ok(s, tr!("commands.worldborder.set.immediate", dist));
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
    /// Border tick (before weather, if the game is running): one-step transition, final size
    /// reached when the remaining ticks drop to zero.
    pub(crate) fn border_tick(&mut self) {
        let b = &mut self.cmd.border;
        let Some((from, to, dur, left)) = b.lerp else { return };
        let left = left - 1;
        let progress = (dur - left) as f64 / dur as f64;
        b.size = if progress < 1.0 { from + progress * (to - from) } else { to };
        b.lerp = (left > 0).then_some((from, to, dur, left));
        self.cmd.border_dirty = true;
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
