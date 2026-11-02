//! S9: `place feature` (configured features of the generator, `rc-worldgen`), `place template`
//! (templates of the server, `tech_blocks.rs`), `place structure` and `place jigsaw` (structure
//! generation of `rc-worldgen`, placed chunk by chunk into the live world; also the jigsaw
//! block's generation).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::{DimGen, WorldData};
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_region::{DecoChunk, FLUID_TICK_NAMES, Region};
use rc_worldgen::structure::Start;
use rc_worldgen::template::{BBox, Mirror, Rot};

use super::src::{Fail, Src};
use super::{Parsed, R, Seg, args};
use crate::blocks::flag;
use crate::server::Server;
use crate::tr;

/// Generation data kept for `place` (loaded on first request).
pub struct PlaceData {
    pub dim: String,
    pub wd: Arc<WorldData>,
    pub dg: Arc<DimGen>,
    /// Generator with the structures enabled (`place structure`, jigsaw), built at first need
    /// even in a world created without structures.
    pub structured: Option<Arc<DimGen>>,
}

impl std::fmt::Debug for PlaceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PlaceData({})", self.dim)
    }
}

impl Server {
    pub(crate) fn place_data(&mut self) -> Option<(Arc<WorldData>, Arc<DimGen>)> {
        if let Some(p) = &self.prog.place
            && p.dim == self.dim.name
        {
            return Some((p.wd.clone(), p.dg.clone()));
        }
        let (spec, seed) = crate::generation::read_spec(&self.world, &self.dim.name)?;
        let jar = crate::registries::client_jar(&self.data)?;
        let reports = crate::generation::reports_dir(&self.data);
        let wd = WorldData::shared(&jar, reports.is_dir().then_some(reports.as_path()), &self.data).ok()?;
        let dg = Arc::new(DimGen::new(&wd, &spec, &self.dim.name, seed).ok()?);
        self.prog.place =
            Some(Box::new(PlaceData { dim: self.dim.name.clone(), wd: wd.clone(), dg: dg.clone(), structured: None }));
        Some((wd, dg))
    }

    /// Generator of the open dimension with its structures (kept with the `place` data).
    fn structured_gen(&mut self) -> Option<(Arc<WorldData>, Arc<DimGen>)> {
        let (wd, _) = self.place_data()?;
        if let Some(g) = self.prog.place.as_ref().and_then(|p| p.structured.clone()) {
            return Some((wd, g));
        }
        let (spec, seed) = crate::generation::read_spec(&self.world, &self.dim.name)?;
        let mut dg = DimGen::new(&wd, &spec, &self.dim.name, seed).ok()?;
        dg.enable_structures(&wd).ok()?;
        let dg = Arc::new(dg);
        if let Some(p) = self.prog.place.as_mut() {
            p.structured = Some(dg.clone());
        }
        Some((wd, dg))
    }

    /// The pieces of a start placed into the live world, one chunk column at a time over the
    /// start's box (like the game), on copies of the chunks with a one-chunk border so that a
    /// chunk sees what its neighbours received; then the changed cells, ticks, post-processing,
    /// block entity data and entities are written back. Randomness drawn from the world's.
    pub(crate) fn place_start_live(&mut self, wd: &WorldData, dg: &DimGen, st: &Start, keep: bool) -> bool {
        let (Some(sg), Some(deco)) = (dg.structures.clone(), dg.deco.clone()) else { return false };
        let bb = st.bbox;
        let (c0, c1) = ((bb.min[0] >> 4, bb.min[2] >> 4), (bb.max[0] >> 4, bb.max[2] >> 4));
        let (gx0, gz0) = (c0.0 - 1, c0.1 - 1);
        let w = c1.0 - c0.0 + 3;
        let h = c1.1 - c0.1 + 3;
        let mut grid: Vec<DecoChunk> = Vec::with_capacity((w * h) as usize);
        for dz in 0..h {
            for dx in 0..w {
                grid.push(self.deco_chunk(gx0 + dx, gz0 + dz, dg, wd));
            }
        }
        let hi = i64::from(self.bs.rng.next_int());
        let mut rng =
            rc_worldgen::decor_rng::DecoRng::legacy((hi << 32).wrapping_add(i64::from(self.bs.rng.next_int())));
        let (min_y, top) = (self.dim.height.min_y, self.top_y());
        let idx = |x: i32, z: i32| ((z - gz0) * w + (x - gx0)) as usize;
        // Chunks in the game's order (x varies first).
        for cz in c0.1..=c1.1 {
            for cx in c0.0..=c1.0 {
                let ids: [usize; 9] = std::array::from_fn(|i| idx(cx + i as i32 % 3 - 1, cz + i as i32 / 3 - 1));
                let Ok(refs) = grid.get_disjoint_mut(ids) else { continue };
                let mut r = Region::new(refs, &wd.blocks, dg.sea_level, dg.seed);
                let mut ctx = rc_worldgen::decor::Ctx::new(&mut r, &deco);
                let zone = BBox::new([cx * 16, min_y, cz * 16], [cx * 16 + 15, top, cz * 16 + 15]);
                rc_worldgen::structure_place::place_start_zone(&mut ctx, &sg, st, &mut rng, zone, keep);
            }
        }
        self.apply_deco(&grid, &wd.blocks);
        for dc in &grid {
            if !matches!(self.chunks.get(&(dc.cx, dc.cz)), Some(Some(_))) {
                continue;
            }
            // Shapes of the marked cells, like the post-processing of a generated chunk.
            for &p in &dc.post_process {
                let s = self.get(p);
                let ns = self.update_from_neighbour_shapes(s, p);
                if ns != s {
                    self.set_block(p, ns, flag::CLIENTS | flag::KNOWN_SHAPE);
                }
            }
            for (p, d) in &dc.be_data {
                if self.be_get(*p).is_some() {
                    let mut f = Vec::new();
                    crate::generation_nbt::be_extra_fields(d, &mut f);
                    self.set_block_data(*p, Nbt::Compound(f));
                }
            }
            let src = self.player_source();
            for e in &dc.entities {
                let ty = e.get("id").and_then(Nbt::as_str).unwrap_or("").to_owned();
                if crate::mobs::species::Kind::from_name(&ty).is_some() {
                    self.next_entity += 1;
                    let n = self.next_entity;
                    let _ = self.adopt_with_passengers(n, e, None);
                } else if let Some(pos) = crate::nbtio::f64s::<3>(e.get("Pos")) {
                    let _ = self.summon_entity(&src, &ty, pos, Some(e.clone()));
                }
            }
        }
        true
    }

    fn chunk_loaded_xz(&self, x: i32, z: i32) -> bool {
        matches!(self.chunks.get(&(x >> 4, z >> 4)), Some(Some(_)))
    }

    /// Decoration chunk copied from a loaded chunk (blocks, biomes, heightmaps).
    pub(crate) fn deco_chunk(&self, cx: i32, cz: i32, dg: &DimGen, wd: &WorldData) -> DecoChunk {
        let h = self.dim.height;
        let mut dc = DecoChunk::new(cx, cz, h.min_y, h.sections as i32 * 16);
        let air = self.states.air_state;
        dc.blocks.fill(air);
        let conv = crate::generation_nbt::Conv::new(&self.data, &self.states, &wd.blocks, h, &dg.biome_names);
        if let Some(Some(c)) = self.chunks.get(&(cx, cz)) {
            for (si, sec) in c.sections.iter().enumerate() {
                sec.blocks.unpack_into(&mut dc.blocks[si * 4096..(si + 1) * 4096]);
                for i in 0..64 {
                    let reg = sec.biomes.get(i);
                    dc.biomes[si * 64 + i] = conv.reg_to_biome.get(&reg).copied().unwrap_or(0);
                }
            }
        }
        for hm in [
            rc_worldgen::decor_region::Hm::WorldSurfaceWg,
            rc_worldgen::decor_region::Hm::WorldSurface,
            rc_worldgen::decor_region::Hm::OceanFloorWg,
            rc_worldgen::decor_region::Hm::OceanFloor,
            rc_worldgen::decor_region::Hm::MotionBlocking,
            rc_worldgen::decor_region::Hm::MotionBlockingNoLeaves,
        ] {
            dc.prime(&wd.blocks, hm);
        }
        dc
    }

    /// Cells that a feature changed in a chunk of its region, with their new state: the region
    /// starts from a copy of the world (`deco_chunk`), which has not moved since. Each section of
    /// the world is unpacked in one pass; an identical section is skipped by one comparison.
    pub(crate) fn deco_changes(&self, dc: &DecoChunk) -> Vec<([i32; 3], u32)> {
        let mut out = Vec::new();
        let Some(Some(c)) = self.chunks.get(&(dc.cx, dc.cz)) else { return out };
        let min_y = self.dim.height.min_y;
        let mut live = [0u32; 4096];
        for (si, (sec, new)) in c.sections.iter().zip(dc.blocks.as_chunks::<4096>().0).enumerate() {
            sec.blocks.unpack_into(&mut live);
            if live == *new {
                continue;
            }
            for (i, (&old, &s)) in live.iter().zip(new).enumerate() {
                if old != s {
                    let (x, z, y) = ((i & 15) as i32, ((i >> 4) & 15) as i32, (i >> 8) as i32);
                    out.push(([dc.cx * 16 + x, min_y + si as i32 * 16 + y, dc.cz * 16 + z], s));
                }
            }
        }
        out
    }

    /// Writes into the world what a feature changed in the chunks of its region ("clients" flag)
    /// and schedules its ticks.
    pub(crate) fn apply_deco(&mut self, chunks: &[DecoChunk], b: &Blocks) -> usize {
        let changes: Vec<_> = chunks.iter().map(|dc| self.deco_changes(dc)).collect();
        // Cells broken with drops by the feature (planted nether fungus): particles, sound and
        // drops of whatever the cell holds at the moment of the break, like the game: a broken
        // weeping vine turns the body above it into a tip, which is broken in turn. A cell
        // already emptied by a previous break (upper half of a double plant) is air, and the
        // break does nothing there.
        for dc in chunks {
            for &(p, _) in &dc.destroyed {
                self.destroy_block(p, true, 512);
            }
        }
        // Only the cells the feature changed are written: elsewhere, the world keeps what the
        // breaks left there (vine tip above the stem, half of a tall grass removed with the
        // other, grass that loses its snow), like the game, which breaks then places cell by
        // cell in the world itself.
        let mut n = 0;
        for &(p, new) in changes.iter().flatten() {
            if self.get(p) != new {
                self.set_block(p, new, flag::CLIENTS);
                n += 1;
            }
        }
        for dc in chunks {
            let Some(Some(_)) = self.chunks.get(&(dc.cx, dc.cz)) else { continue };
            // Ticks scheduled by the feature (sources, geode cracks, flowers…), with the requested
            // delay: in the live world, it is not brought back to 0.
            for &(p, k) in &dc.fluid_ticks {
                let name = FLUID_TICK_NAMES[usize::from(k)];
                if let Some(f) = crate::blocks::ticks::FLUID_NAMES.iter().position(|n| *n == name) {
                    self.schedule_fluid_tick(p, f as u8, 0);
                }
            }
            for &(p, k, delay) in &dc.block_ticks {
                let id = self.bs.t.block_id(b.block_name(k));
                if id != u16::MAX {
                    self.schedule_tick(p, id, delay);
                }
            }
        }
        n
    }

    pub(crate) fn place_feature_cmd(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let id = args::id(&p.arg(seg, "feature").unwrap_or_default());
        let pos = match p.arg(seg, "pos") {
            Some(t) => args::block_pos(self, s, &t),
            None => crate::mobs::mth::block_of(s.pos),
        };
        // The 3×3 chunks around the position must be loaded (the feature may write into them).
        let loaded = (-1..=1).all(|dz| (-1..=1).all(|dx| self.chunk_loaded_xz(pos[0] + dx * 16, pos[2] + dz * 16)));
        if !loaded {
            return Err(Fail(tr!("argument.pos.unloaded")));
        }
        let Some((wd, dg)) = self.place_data() else { return Err(Fail(tr!("commands.place.feature.failed"))) };
        let Some(deco) = dg.deco.clone() else { return Err(Fail(tr!("commands.place.feature.failed"))) };
        let Some(fid) = deco.reg.feature_id(&id) else {
            return Err(Fail(tr!("argument.resource.not_found", id, "minecraft:worldgen/configured_feature")));
        };
        let (cx, cz) = (pos[0] >> 4, pos[2] >> 4);
        let mut chunks: Vec<DecoChunk> = Vec::with_capacity(9);
        for dz in -1..=1 {
            for dx in -1..=1 {
                chunks.push(self.deco_chunk(cx + dx, cz + dz, &dg, &wd));
            }
        }
        // Drawn on the world's random source (like the bone meal features), not the clock.
        let hi = i64::from(self.bs.rng.next_int());
        let seed = (hi << 32).wrapping_add(i64::from(self.bs.rng.next_int()));
        let placed = {
            let mut it = chunks.iter_mut();
            let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().expect("9 chunks"));
            let mut r = Region::new(refs, &wd.blocks, dg.sea_level, dg.seed);
            let mut ctx = rc_worldgen::decor::Ctx::new(&mut r, &deco);
            let mut rng = rc_worldgen::decor_rng::DecoRng::legacy(seed);
            rc_worldgen::feature::place_feature(&mut ctx, fid, &mut rng, pos)
        };
        if !placed {
            return Err(Fail(tr!("commands.place.feature.failed")));
        }
        self.apply_deco(&chunks, &wd.blocks);
        self.ok(s, tr!("commands.place.feature.success", id, pos[0], pos[1], pos[2]));
        Ok(1)
    }

    pub(crate) fn place_template(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let id = args::id(&p.arg(seg, "template").unwrap_or_default());
        let pos = match p.arg(seg, "pos") {
            Some(t) => args::block_pos(self, s, &t),
            None => crate::mobs::mth::block_of(s.pos),
        };
        let rot = match p.arg(seg, "rotation").as_deref() {
            Some("clockwise_90") => Rot::Cw90,
            Some("180") => Rot::Cw180,
            Some("counterclockwise_90") => Rot::Ccw90,
            _ => Rot::None,
        };
        let mirror = match p.arg(seg, "mirror").as_deref() {
            Some("left_right") => Mirror::LeftRight,
            Some("front_back") => Mirror::FrontBack,
            _ => Mirror::None,
        };
        let integrity: f32 = p.arg(seg, "integrity").and_then(|v| v.parse().ok()).unwrap_or(1.0);
        let seed: i32 = p.arg(seg, "seed").and_then(|v| v.parse().ok()).unwrap_or(0);
        let strict = seg.path.iter().any(|x| x == "strict");
        // Template of the server (world folder `generated/<ns>/structure/`, game files, captures).
        let tpl = crate::tech_blocks::template_id(&id).and_then(|i| self.template_get(&i));
        let Some(t) = tpl.map(|e| e.t) else { return Err(Fail(tr!("commands.place.template.invalid", id))) };
        let far = [pos[0] + t.size[0], pos[1] + t.size[1], pos[2] + t.size[2]];
        for cx in (pos[0].min(far[0]) >> 4)..=(pos[0].max(far[0]) >> 4) {
            for cz in (pos[2].min(far[2]) >> 4)..=(pos[2].max(far[2]) >> 4) {
                if !matches!(self.chunks.get(&(cx, cz)), Some(Some(_))) {
                    return Err(Fail(tr!("argument.pos.unloaded")));
                }
            }
        }
        // Placement of the game (palette drawn, integrity on the settings' own random, entities,
        // waterlogging, shape pass outside strict mode).
        let set = crate::tech_blocks::LivePlace {
            mirror,
            rot,
            ignore_entities: false,
            known_shape: strict,
            integrity,
            seed: i64::from(seed),
        };
        let flags = flag::CLIENTS | if strict { 816 } else { 0 };
        if !self.place_template_live(&t, pos, &set, flags) {
            return Err(Fail(tr!("commands.place.template.failed")));
        }
        self.ok(s, tr!("commands.place.template.success", id, pos[0], pos[1], pos[2]));
        Ok(1)
    }

    /// `place structure <structure> [<pos>]`: start generated at the chunk of the position
    /// (placement rules and biome ignored), every chunk of its box loaded, then placed.
    pub(crate) fn place_structure_cmd(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let id = args::id(&p.arg(seg, "structure").unwrap_or_default());
        let pos = match p.arg(seg, "pos") {
            Some(t) => args::block_pos(self, s, &t),
            None => crate::mobs::mth::block_of(s.pos),
        };
        let Some((wd, dg)) = self.structured_gen() else { return Err(Fail(tr!("commands.place.structure.failed"))) };
        if !dg.structures.as_ref().is_some_and(|sg| sg.defs.iter().any(|d| d.id == id)) {
            return Err(Fail(tr!("commands.place.structure.invalid", id)));
        }
        let Some(st) = dg.structure_start_anywhere(&id, pos[0] >> 4, pos[2] >> 4) else {
            return Err(Fail(tr!("commands.place.structure.failed")));
        };
        let bb = st.bbox;
        let loaded = ((bb.min[0] >> 4)..=(bb.max[0] >> 4))
            .all(|cx| ((bb.min[2] >> 4)..=(bb.max[2] >> 4)).all(|cz| self.chunk_loaded_xz(cx * 16, cz * 16)));
        if !loaded {
            return Err(Fail(tr!("argument.pos.unloaded")));
        }
        self.place_start_live(&wd, &dg, &st, false);
        self.ok(s, tr!("commands.place.structure.success", id, pos[0], pos[1], pos[2]));
        Ok(1)
    }

    /// `place jigsaw <pool> <target> <max_depth> [<position>]`: assembly from the pool at the
    /// position (its chunk loaded).
    pub(crate) fn place_jigsaw_cmd(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let pool = args::id(&p.arg(seg, "pool").unwrap_or_default());
        let target = args::id(&p.arg(seg, "target").unwrap_or_default());
        let depth: i32 = p.arg(seg, "max_depth").and_then(|v| v.parse().ok()).unwrap_or(1);
        let pos = match p.arg(seg, "position") {
            Some(t) => args::block_pos(self, s, &t),
            None => crate::mobs::mth::block_of(s.pos),
        };
        if !self.chunk_loaded_xz(pos[0], pos[2]) {
            return Err(Fail(tr!("argument.pos.unloaded")));
        }
        match self.generate_jigsaw(&pool, &target, depth, pos, false) {
            Some(true) => {
                self.ok(s, tr!("commands.place.jigsaw.success", pos[0], pos[1], pos[2]));
                Ok(1)
            }
            Some(false) => Err(Fail(tr!("commands.place.jigsaw.failed"))),
            None => Err(Fail(tr!("commands.place.jigsaw.invalid", pool))),
        }
    }

    /// Jigsaw assembly into the live world (command, jigsaw block): `None` for an unknown pool,
    /// `Some(false)` if nothing could start.
    pub(crate) fn generate_jigsaw(
        &mut self,
        pool: &str,
        target: &str,
        depth: i32,
        pos: [i32; 3],
        keep: bool,
    ) -> Option<bool> {
        let (wd, dg) = self.structured_gen()?;
        dg.structures.as_ref()?.pools.get(pool)?;
        let Some(st) = dg.jigsaw_start(pool, target, depth, pos) else { return Some(false) };
        Some(self.place_start_live(&wd, &dg, &st, keep))
    }
}
