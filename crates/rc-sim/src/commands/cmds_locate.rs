//! `locate structure|biome`: generator of the open dimension built on demand (same
//! settings as the generation thread), search like the game — concentric rings for the nearest,
//! random scattering by rings of sectors up to 100, biomes in a spiral of
//! step 32 over a radius of 6,400 with alternating heights around the source (step 64).

use std::collections::BTreeMap;
use std::sync::Arc;

use rc_worldgen::chunkgen::{DimGen, WorldData};
use rc_worldgen::decor_rng::DecoRng;
use rc_worldgen::structure::Placement;

use super::src::{Fail, Src};
use super::text::{self, GREEN, colored};
use super::{Parsed, R, Seg, args};
use crate::server::Server;
use crate::tr;

/// Structure data read from the jar: locating offsets of the sets, tags.
#[derive(Default)]
pub struct LocateData {
    pub offsets: BTreeMap<String, [i32; 3]>,
    pub tags: BTreeMap<String, Vec<String>>,
    /// References placed by the exploration maps during the session: (dimension,
    /// chunk, structure) → count; rewritten into the chunks that go through the server.
    pub refs: hashbrown::HashMap<(String, i32, i32, String), i32>,
}

/// A structure can only be designated by a map.
const MAX_REFERENCES: i32 = 1;

/// References recorded for a structure start in a chunk's compound (`None`: start absent).
fn saved_references(root: &rc_proto::nbt::Nbt, id: &str) -> Option<i32> {
    let st = root.get("structures")?.get("starts")?.get(id)?;
    if st.get("id").and_then(rc_proto::nbt::Nbt::as_str) == Some("INVALID") {
        return None;
    }
    Some(crate::nbtio::int(st, "references").unwrap_or(0) as i32)
}

/// Writes the reference count of a start present in the compound; true if changed.
fn set_references(root: &mut rc_proto::nbt::Nbt, id: &str, n: i32) -> bool {
    let Some(st) = crate::nbtio::get_mut(root, "structures")
        .and_then(|s| crate::nbtio::get_mut(s, "starts"))
        .and_then(|s| crate::nbtio::get_mut(s, id))
    else {
        return false;
    };
    if crate::nbtio::int(st, "references") == Some(i64::from(n)) {
        return false;
    }
    crate::nbtio::set(st, "references", rc_proto::nbt::Nbt::Int(n));
    true
}

/// Adds to the `structures` compound of a chunk on disk the starts and references of `fresh`
/// that it does not have yet (those already recorded stay as they are).
fn merge_structures(root: &mut rc_proto::nbt::Nbt, fresh: rc_proto::nbt::Nbt) {
    use rc_proto::nbt::Nbt;
    let Nbt::Compound(fresh) = fresh else { return };
    let Nbt::Compound(fields) = root else { return };
    let Some(pos) = fields.iter().position(|(k, _)| k == "structures") else {
        fields.push(("structures".into(), Nbt::Compound(fresh)));
        return;
    };
    let Nbt::Compound(old) = &mut fields[pos].1 else {
        fields[pos].1 = Nbt::Compound(fresh);
        return;
    };
    for (section, v) in fresh {
        let Nbt::Compound(items) = v else { continue };
        match old.iter_mut().find(|(k, _)| *k == section) {
            Some((_, Nbt::Compound(have))) => {
                for (id, item) in items {
                    if !have.iter().any(|(k, _)| *k == id) {
                        have.push((id, item));
                    }
                }
            }
            _ => old.push((section, Nbt::Compound(items))),
        }
    }
}

impl LocateData {
    pub(crate) fn read(jar: &crate::registries::Jar) -> LocateData {
        let mut d = LocateData::default();
        for (k, bytes) in &jar.files {
            let Some(rest) = k.strip_prefix("data/") else { continue };
            let Some((ns, path)) = rest.split_once('/') else { continue };
            if let Some(set) = path.strip_prefix("worldgen/structure_set/").and_then(|p| p.strip_suffix(".json")) {
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { continue };
                if let Some(a) = v.pointer("/placement/locate_offset").and_then(serde_json::Value::as_array) {
                    let g = |i: usize| a.get(i).and_then(serde_json::Value::as_i64).unwrap_or(0) as i32;
                    d.offsets.insert(format!("{ns}:{set}"), [g(0), g(1), g(2)]);
                }
            } else if let Some(tag) =
                path.strip_prefix("tags/worldgen/structure/").and_then(|p| p.strip_suffix(".json"))
            {
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { continue };
                let vals = v
                    .get("values")
                    .and_then(serde_json::Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().or_else(|| x.get("id").and_then(serde_json::Value::as_str)))
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default();
                d.tags.insert(format!("{ns}:{tag}"), vals);
            }
        }
        d
    }

    /// Members of a tag (nested tags expanded).
    fn members(&self, tag: &str, depth: u32) -> Vec<String> {
        let mut out = Vec::new();
        for v in self.tags.get(tag).into_iter().flatten() {
            match v.strip_prefix('#') {
                Some(t) if depth < 8 => out.extend(self.members(&args::id(t), depth + 1)),
                Some(_) => {}
                None => out.push(args::id(v)),
            }
        }
        out
    }
}

/// The spiral walk around a position (first direction east, second south): offsets (x, z), centre included.
pub fn spiral(radius: i32) -> Vec<(i32, i32)> {
    let dirs = [(1, 0), (0, 1), (-1, 0), (0, -1)];
    let mut out = Vec::new();
    let (mut x, mut z) = (0, 1);
    let legs = 4 * radius;
    let (mut leg, mut leg_size, mut leg_index) = (-1i32, 0, 0);
    loop {
        let d = dirs[((leg + 4) % 4) as usize];
        x += d.0;
        z += d.1;
        if leg_index >= leg_size {
            if leg >= legs {
                break;
            }
            leg += 1;
            leg_index = 0;
            leg_size = leg / 2 + 1;
        }
        leg_index += 1;
        out.push((x, z));
    }
    out
}

/// Clamped origin then alternating below / above by step.
pub fn out_from_origin(origin: i32, lo: i32, hi: i32, step: i32) -> Vec<i32> {
    let o = origin.clamp(lo, hi);
    let mut out = Vec::new();
    let mut cur = o;
    loop {
        let dist = (o - cur).abs();
        if !(o - dist >= lo || o + dist <= hi) {
            break;
        }
        out.push(cur);
        let prev_neg = cur <= o;
        let can_pos = o + dist + step <= hi;
        let mut next = None;
        if !prev_neg || !can_pos {
            let attempt = o - dist - if prev_neg { step } else { 0 };
            if attempt >= lo {
                next = Some(attempt);
            }
        }
        cur = next.unwrap_or(o + dist + step);
        if out.len() > 100_000 {
            break;
        }
    }
    out
}

impl Server {
    /// Generator of the open dimension for the searches (built once).
    fn locate_gen(&mut self) -> Option<Arc<DimGen>> {
        if let Some((d, g)) = &self.cmd.locgen
            && *d == self.dim.name
        {
            return Some(g.clone());
        }
        let (spec, seed) = crate::generation::read_spec(&self.world, &self.dim.name)?;
        let jar = crate::registries::client_jar(&self.data)?;
        let reports = crate::generation::reports_dir(&self.data);
        let wd = WorldData::shared(&jar, reports.is_dir().then_some(reports.as_path()), &self.data).ok()?;
        let mut dg = DimGen::new(&wd, &spec, &self.dim.name, seed).ok()?;
        if crate::generation::structures_enabled(&self.world) {
            dg.enable_structures(&wd).ok()?;
        }
        let g = Arc::new(dg);
        self.cmd.locgen = Some((self.dim.name.clone(), g.clone()));
        Some(g)
    }

    pub(crate) fn cmd_locate(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let what = seg.path.get(1).cloned().unwrap_or_default();
        let origin = [s.pos[0].floor() as i32, s.pos[1].floor() as i32, s.pos[2].floor() as i32];
        match what.as_str() {
            "structure" => self.locate_structure(s, &p.arg(seg, "structure").unwrap_or_default(), origin),
            "biome" => self.locate_biome(s, &p.arg(seg, "biome").unwrap_or_default(), origin),
            _ => Err(Fail(tr!("commands.locate.poi.not_found", p.arg(seg, "poi").unwrap_or_default()))),
        }
    }

    fn locate_result(&mut self, s: &Src, key: &str, name: String, origin: [i32; 3], found: [i32; 3], y: bool) -> R {
        let dist = if y {
            let d = (0..3).map(|k| f64::from(found[k] - origin[k]).powi(2)).sum::<f64>();
            (d as f32).sqrt().floor() as i32
        } else {
            let (dx, dz) = (found[0] - origin[0], found[2] - origin[2]);
            ((dx * dx + dz * dz) as f32).sqrt().floor() as i32
        };
        let ys = if y { found[1].to_string() } else { "~".to_owned() };
        let mut coords = colored(text::brackets(tr!("chat.coordinates", found[0], ys.clone(), found[2])), GREEN);
        text::suggest_click(&mut coords, format!("/tp @s {} {} {}", found[0], ys, found[2]));
        text::hover_text(&mut coords, tr!("chat.coordinates.tooltip"));
        self.ok(s, tr!(key, name, coords, dist));
        Ok(dist)
    }

    fn locate_structure(&mut self, s: &Src, arg: &str, origin: [i32; 3]) -> R {
        let (is_tag, id) = match arg.strip_prefix('#') {
            Some(t) => (true, args::id(t)),
            None => (false, args::id(arg)),
        };
        let printable = if is_tag { format!("#{id}") } else { id.clone() };
        let wanted: Vec<String> = if is_tag { self.cmd.locate.members(&id, 0) } else { vec![id.clone()] };
        if wanted.is_empty() {
            return Err(Fail(tr!("commands.locate.structure.invalid", printable)));
        }
        let Some((p, sid)) = self.structure_search(&wanted, origin, 100) else {
            return Err(Fail(tr!("commands.locate.structure.not_found", printable)));
        };
        let name = if is_tag { format!("{printable} ({sid})") } else { printable };
        self.locate_result(s, "commands.locate.structure.success", name, origin, p, false)
    }

    /// Nearest structure of a tag (radius in chunks;
    /// F09: dolphin treasures).
    pub(crate) fn nearest_structure(&mut self, tag: &str, origin: [i32; 3], radius: i32) -> Option<[i32; 3]> {
        let id = args::id(tag.trim_start_matches('#'));
        let wanted = self.cmd.locate.members(&id, 0);
        if wanted.is_empty() {
            return None;
        }
        self.structure_search(&wanted, origin, radius).map(|r| r.0)
    }

    /// S8: generator biomes (biome registry indices) at the given positions
    /// (exploration map preview).
    pub(crate) fn generated_biomes(&mut self, points: &[[i32; 3]]) -> Vec<Option<usize>> {
        let names = self.regs.entries("minecraft:worldgen/biome").to_vec();
        let Some(g) = self.locate_gen() else { return vec![None; points.len()] };
        let Some(t) = g.terrain_generator() else {
            let flat = g.biome_names.first().cloned().unwrap_or_default();
            let i = names.iter().position(|n| *n == flat);
            return vec![i; points.len()];
        };
        let mut ctx = rc_worldgen::sampler::Ctx::cached();
        let mut search = rc_worldgen::biome::Searcher::default();
        points
            .iter()
            .map(|p| {
                let b = t.source.biome_at(&t.g, &mut ctx, &mut search, p[0] >> 2, p[1] >> 2, p[2] >> 2);
                let n = t.biome_names.get(usize::from(b))?;
                names.iter().position(|x| x == n)
            })
            .collect()
    }

    /// S8: structures of a tag (or identifier) for the exploration maps.
    pub(crate) fn structure_tag_members(&self, tag_or_id: &str) -> Vec<String> {
        match tag_or_id.strip_prefix('#') {
            Some(t) => self.cmd.locate.members(&args::id(t), 0),
            None => vec![args::id(tag_or_id)],
        }
    }

    /// Common search (`locate`, S8 exploration maps, F09 dolphin treasures): rings,
    /// origin, then random placements by growing rings up to `radius`; reference position
    /// and structure identifier.
    pub(crate) fn structure_search(
        &mut self,
        wanted: &[String],
        origin: [i32; 3],
        radius: i32,
    ) -> Option<([i32; 3], String)> {
        self.structure_search_ref(wanted, origin, radius, false)
    }

    /// Like [`Self::structure_search`]; `create_reference` (exploration maps that skip
    /// known structures): a start already designated is passed over, the one retained at each
    /// step of the search receives a reference (like the game, including the nearer candidates
    /// found afterwards on the rings).
    pub(crate) fn structure_search_ref(
        &mut self,
        wanted: &[String],
        origin: [i32; 3],
        radius: i32,
        create_reference: bool,
    ) -> Option<([i32; 3], String)> {
        let g = self.locate_gen()?;
        let (Some(sg), Some(t)) = (g.structures.clone(), g.terrain_generator()) else { return None };
        let idx: Vec<usize> = wanted.iter().filter_map(|w| sg.defs.iter().position(|d| d.id == *w)).collect();
        // Sets concerned, in registry order.
        let sets: Vec<usize> =
            (0..sg.sets.len()).filter(|&i| sg.sets[i].structures.iter().any(|(st, _)| idx.contains(st))).collect();
        if sets.is_empty() {
            return None;
        }
        let offsets: Vec<[i32; 3]> =
            sg.sets.iter().map(|s| self.cmd.locate.offsets.get(&s.id).copied().unwrap_or([0; 3])).collect();
        let mut start_at = |cx: i32, cz: i32| -> Option<usize> {
            let starts = g.structure_starts(cx, cz);
            let st = starts.iter().find(|st| idx.contains(&st.structure)).map(|st| st.structure)?;
            if create_reference && !self.try_add_structure_reference(&g, cx, cz, &sg.defs[st].id) {
                return None;
            }
            Some(st)
        };
        let locate_pos = |set: usize, cx: i32, cz: i32| -> [i32; 3] {
            let off = offsets[set];
            [cx * 16 + off[0], off[1], cz * 16 + off[2]]
        };
        let d2 = |p: [i32; 3]| (0..3).map(|k| (f64::from(p[k]) - f64::from(origin[k])).powi(2)).sum::<f64>();
        let mut best: Option<([i32; 3], usize)> = None;
        let mut best_d = f64::MAX;
        let mut randoms = Vec::new();
        for &set in &sets {
            match &sg.sets[set].placement {
                Placement::Rings { .. } => {
                    let ring = sg.ring_positions(set, t);
                    let mut closest: Option<([i32; 3], usize)> = None;
                    let mut min = f64::MAX;
                    for &(cx, cz) in ring.iter() {
                        let p = [cx * 16 + 8, 32, cz * 16 + 8];
                        let d = d2(p);
                        if (closest.is_none() || d < min)
                            && let Some(st) = start_at(cx, cz)
                        {
                            closest = Some((locate_pos(set, cx, cz), st));
                            min = d;
                        }
                    }
                    if let Some((p, st)) = closest {
                        let d = d2(p);
                        if d < best_d {
                            best_d = d;
                            best = Some((p, st));
                        }
                    }
                }
                Placement::Random { .. } => randoms.push(set),
                Placement::Origin => {
                    if let Some(st) = start_at(0, 0) {
                        let p = locate_pos(set, 0, 0);
                        let d = d2(p);
                        if d < best_d {
                            best_d = d;
                            best = Some((p, st));
                        }
                    }
                }
            }
        }
        if !randoms.is_empty() {
            let (ox, oz) = (origin[0] >> 4, origin[2] >> 4);
            'rings: for r in 0..=radius {
                let mut found_any = false;
                for &set in &randoms {
                    let Placement::Random { spacing, separation, triangular, salt, .. } = &sg.sets[set].placement
                    else {
                        continue;
                    };
                    let mut hit = None;
                    'ring: for x in -r..=r {
                        let x_edge = x == -r || x == r;
                        for z in -r..=r {
                            if !(x_edge || z == -r || z == r) {
                                continue;
                            }
                            let (sx, sz) = (ox + spacing * x, oz + spacing * z);
                            let (cx, cz) = potential_chunk(sg.seed, sx, sz, *spacing, *separation, *triangular, *salt);
                            if sg.is_structure_chunk(set, cx, cz, t)
                                && let Some(st) = start_at(cx, cz)
                            {
                                hit = Some((locate_pos(set, cx, cz), st));
                                break 'ring;
                            }
                        }
                    }
                    if let Some((p, st)) = hit {
                        found_any = true;
                        let d = d2(p);
                        if d < best_d {
                            best_d = d;
                            best = Some((p, st));
                        }
                    }
                }
                if found_any {
                    break 'rings;
                }
            }
        }
        best.map(|(p, st)| (p, sg.defs[st].id.clone()))
    }

    /// Current references of a start: session, loaded chunk, otherwise disk (0 if nothing).
    fn structure_references(&mut self, cx: i32, cz: i32, id: &str) -> i32 {
        let key = (self.dim.name.clone(), cx, cz, id.to_owned());
        if let Some(&n) = self.cmd.locate.refs.get(&key) {
            return n;
        }
        match self.chunks.get(&(cx, cz)) {
            Some(Some(c)) => saved_references(&c.root, id),
            _ => self
                .dim
                .load_raw("region", &[(cx, cz)])
                .pop()
                .and_then(|(_, n)| n)
                .and_then(|r| saved_references(&r, id)),
        }
        .unwrap_or(0)
    }

    /// False if the start is already designated; otherwise one more reference,
    /// recorded in the chunk (loaded, on disk, or "structure starts" proto-chunk
    /// written for it, like the game which loads the chunk up to that status).
    fn try_add_structure_reference(&mut self, g: &DimGen, cx: i32, cz: i32, id: &str) -> bool {
        let n = self.structure_references(cx, cz, id);
        if n >= MAX_REFERENCES {
            return false;
        }
        let n = n + 1;
        self.cmd.locate.refs.insert((self.dim.name.clone(), cx, cz, id.to_owned()), n);
        if let Some(Some(c)) = self.chunks.get_mut(&(cx, cz)) {
            if set_references(&mut c.root, id, n) {
                c.dirty = true;
            }
            return true;
        }
        use rc_proto::nbt::Nbt;
        let disk = self.dim.load_raw("region", &[(cx, cz)]).pop().and_then(|(_, n)| n);
        let status = disk.as_ref().and_then(|r| r.get("Status")).and_then(Nbt::as_str).map(str::to_owned);
        let root = match disk {
            Some(mut r) if saved_references(&r, id).is_some() => {
                set_references(&mut r, id, n);
                Some(r)
            }
            // Absent, or empty proto-chunk: structure starts placed with the reference.
            None => Some(self.structure_starts_proto(g, cx, cz)),
            // Empty chunk from disk (old converted chunk included): its sections and
            // `below_zero_retrogen` stay, only the missing starts are added.
            Some(mut r) if matches!(status.as_deref(), Some("minecraft:empty" | "empty")) => {
                let retro = r.get("below_zero_retrogen").is_some();
                let fresh = if retro { g.structures_nbt_retro(cx, cz) } else { g.structures_nbt(cx, cz) };
                merge_structures(&mut r, fresh);
                let bare = !matches!(r.get("sections"), Some(Nbt::List(l)) if !l.is_empty());
                if bare && !retro {
                    crate::nbtio::set(&mut r, "Status", Nbt::String("minecraft:structure_starts".into()));
                }
                Some(r)
            }
            Some(_) => None,
        };
        if let Some(mut r) = root {
            set_references(&mut r, id, n);
            let job = crate::save::Job::Chunk {
                path: self.dim.region_dir().join(crate::region::region_name(cx, cz)),
                cx,
                cz,
                nbt: crate::nbtio::encode_root(&r),
            };
            self.writer.get_or_insert_with(crate::save::Writer::new).send(vec![job]);
        }
        true
    }

    /// Proto-chunk at the "structure starts" status (nothing else generated).
    fn structure_starts_proto(&self, g: &DimGen, cx: i32, cz: i32) -> rc_proto::nbt::Nbt {
        use rc_proto::nbt::Nbt;
        Nbt::Compound(vec![
            ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ("xPos".into(), Nbt::Int(cx)),
            ("yPos".into(), Nbt::Int(self.dim.height.min_section())),
            ("zPos".into(), Nbt::Int(cz)),
            ("LastUpdate".into(), Nbt::Long(self.time)),
            ("InhabitedTime".into(), Nbt::Long(0)),
            ("Status".into(), Nbt::String("minecraft:structure_starts".into())),
            ("structures".into(), g.structures_nbt(cx, cz)),
            ("sections".into(), Nbt::List(Vec::new())),
        ])
    }

    /// Session references rewritten into a full chunk arrived from the generator.
    pub(crate) fn apply_structure_refs(&mut self, p: (i32, i32)) {
        let Some(Some(c)) = self.chunks.get_mut(&p) else { return };
        for ((d, x, z, id), n) in &self.cmd.locate.refs {
            if *d == self.dim.name && (*x, *z) == p && set_references(&mut c.root, id, *n) {
                c.dirty = true;
            }
        }
    }

    /// Same for an encoded proto-chunk to write (returned as is if not concerned).
    pub(crate) fn patch_proto_refs(&self, p: (i32, i32), nbt: Vec<u8>) -> Vec<u8> {
        let mine: Vec<(&String, i32)> = self
            .cmd
            .locate
            .refs
            .iter()
            .filter(|((d, x, z, _), _)| *d == self.dim.name && (*x, *z) == p)
            .map(|((_, _, _, id), n)| (id, *n))
            .collect();
        if mine.is_empty() {
            return nbt;
        }
        let Ok(mut root) = crate::nbtio::parse_root(&nbt) else { return nbt };
        let mut changed = false;
        for (id, n) in mine {
            changed |= set_references(&mut root, id, n);
        }
        if changed { crate::nbtio::encode_root(&root) } else { nbt }
    }

    fn locate_biome(&mut self, s: &Src, arg: &str, origin: [i32; 3]) -> R {
        let (is_tag, id) = match arg.strip_prefix('#') {
            Some(t) => (true, args::id(t)),
            None => (false, args::id(arg)),
        };
        let printable = if is_tag { format!("#{id}") } else { id.clone() };
        let names = self.regs.entries("minecraft:worldgen/biome").to_vec();
        let ok: Vec<bool> =
            names
                .iter()
                .map(|n| {
                    if is_tag {
                        self.cmd.data.tag_members("minecraft:worldgen/biome", &id).contains(n)
                    } else {
                        *n == id
                    }
                })
                .collect();
        let Some(g) = self.locate_gen() else {
            return Err(Fail(tr!("commands.locate.biome.not_found", printable)));
        };
        let min_y = self.dim.height.min_y;
        let max_y = min_y + self.dim.height.sections as i32 * 16 - 1;
        let ys = out_from_origin(origin[1], min_y + 1, max_y + 1, 64);
        let Some(t) = g.terrain_generator() else {
            // Flat world: a single biome.
            let flat = g.biome_names.first().cloned().unwrap_or_default();
            let hit = names.iter().position(|n| *n == flat).is_some_and(|i| ok[i]);
            if !hit {
                return Err(Fail(tr!("commands.locate.biome.not_found", printable)));
            }
            let name = if is_tag { format!("{printable} ({flat})") } else { printable };
            let found = [origin[0], ys.first().copied().unwrap_or(origin[1]), origin[2]];
            return self.locate_result(s, "commands.locate.biome.success", name, origin, found, true);
        };
        let gen_ok: Vec<bool> =
            t.biome_names.iter().map(|n| names.iter().position(|x| x == n).is_some_and(|i| ok[i])).collect();
        if !gen_ok.iter().any(|&b| b) {
            return Err(Fail(tr!("commands.locate.biome.not_found", printable)));
        }
        let cols = spiral(6400 / 32);
        let pos = |i: usize| {
            let (dx, dz) = cols[i / ys.len()];
            [origin[0] + dx * 32, ys[i % ys.len()], origin[2] + dz * 32]
        };
        let mut search = rc_worldgen::biome::Searcher::default();
        let paths = rc_worldgen::biome::SplitPaths::current();
        let quart = |i: usize| pos(i).map(|v| v >> 2);
        let Some((i, b)) = t.source.first_match(&t.g, cols.len() * ys.len(), quart, &gen_ok, &mut search, paths) else {
            return Err(Fail(tr!("commands.locate.biome.not_found", printable)));
        };
        let bn = t.biome_names.get(usize::from(b)).cloned().unwrap_or_default();
        let name = if is_tag { format!("{printable} ({bn})") } else { printable };
        self.locate_result(s, "commands.locate.biome.success", name, origin, pos(i), true)
    }
}

/// Candidate chunk of a sector.
fn potential_chunk(
    seed: i64,
    x: i32,
    z: i32,
    spacing: i32,
    separation: i32,
    triangular: bool,
    salt: i32,
) -> (i32, i32) {
    let (gx, gz) = (x.div_euclid(spacing), z.div_euclid(spacing));
    let mut rng = DecoRng::legacy(0);
    rng.set_seed(
        i64::from(gx)
            .wrapping_mul(341_873_128_712)
            .wrapping_add(i64::from(gz).wrapping_mul(132_897_987_541))
            .wrapping_add(seed)
            .wrapping_add(i64::from(salt)),
    );
    let l = spacing - separation;
    let mut spread = || {
        if triangular { (rng.next_int_bounded(l) + rng.next_int_bounded(l)) / 2 } else { rng.next_int_bounded(l) }
    };
    let ox = spread();
    let oz = spread();
    (gx * spacing + ox, gz * spacing + oz)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn structures(items: &[(&str, i32)]) -> rc_proto::nbt::Nbt {
        use rc_proto::nbt::Nbt;
        let starts = items
            .iter()
            .map(|(id, r)| ((*id).to_owned(), Nbt::Compound(vec![("references".into(), Nbt::Int(*r))])))
            .collect();
        Nbt::Compound(vec![("References".into(), Nbt::Compound(Vec::new())), ("starts".into(), Nbt::Compound(starts))])
    }

    /// Old chunk from disk: its sections and starts stay, only the missing
    /// starts are added.
    #[test]
    fn merge_keeps_saved_starts_and_sections() {
        use rc_proto::nbt::Nbt;
        let mut root = Nbt::Compound(vec![
            ("sections".into(), Nbt::List(vec![Nbt::Compound(Vec::new())])),
            ("structures".into(), structures(&[("a", 1)])),
        ]);
        merge_structures(&mut root, structures(&[("a", 0), ("b", 0)]));
        assert_eq!(saved_references(&root, "a"), Some(1));
        assert_eq!(saved_references(&root, "b"), Some(0));
        assert!(matches!(root.get("sections"), Some(Nbt::List(l)) if l.len() == 1));
    }

    /// Treasure maps that skip known structures: two searches in a row lead to two
    /// different treasures, the reference is written into the start's chunk and survives
    /// the closing of the world; without skipping, the search gives the nearest again.
    #[test]
    fn exploration_maps_reference_each_start_once() {
        let Some((mut s, dir)) = crate::portal_tests::open_new_world("w12-refs", 20_261_018) else { return };
        let wanted = vec!["minecraft:buried_treasure".to_owned()];
        let origin = [0, 64, 0];
        let plain = s.structure_search_ref(&wanted, origin, 100, false).expect("treasure").0;
        let a = s.structure_search_ref(&wanted, origin, 100, true).expect("first").0;
        assert_eq!(a, plain, "the first one designated is the nearest");
        let b = s.structure_search_ref(&wanted, origin, 100, true).expect("second").0;
        assert_ne!(a, b, "a treasure already designated is skipped");
        assert_eq!(s.structure_search_ref(&wanted, origin, 100, false).expect("without skipping").0, plain);
        s.save(true);
        drop(s);
        let (cx, cz) = (a[0] >> 4, a[2] >> 4);
        let dim = crate::world::Dimension::new(
            &dir,
            "minecraft:overworld",
            crate::dimension_height(&rc_testdata::version_data().unwrap(), "minecraft:overworld"),
        );
        let root = dim.load_raw("region", &[(cx, cz)]).pop().and_then(|(_, n)| n).expect("chunk written");
        assert_eq!(saved_references(&root, "minecraft:buried_treasure"), Some(1));
        let data = std::sync::Arc::new(rc_testdata::version_data().unwrap());
        let mut s = crate::portal_tests::open_world(&dir, data);
        assert_eq!(s.structure_search_ref(&wanted, origin, 100, false).map(|r| r.0), Some(plain), "read back");
        let c = s.structure_search_ref(&wanted, origin, 100, true).expect("third").0;
        assert!(c != a && c != b, "references read back from disk: {a:?} {b:?} {c:?}");
        drop(s);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spiral_and_heights() {
        let s = spiral(1);
        assert_eq!(&s[..5], &[(0, 0), (1, 0), (1, 1), (0, 1), (-1, 1)]);
        assert_eq!(s.len(), 9);
        assert_eq!(out_from_origin(70, -63, 320, 64)[..4], [70, 134, 6, 198]);
    }
}
