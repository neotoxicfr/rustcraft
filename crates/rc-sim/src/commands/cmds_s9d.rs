//! S9: `place feature` (configured features of the generator, `rc-worldgen`), `place template`
//! (templates of the server, `tech_blocks.rs`), `place structure` and `place jigsaw` (structure
//! generation of `rc-worldgen`, placed chunk by chunk into the live world; also the jigsaw
//! block's generation).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::{DimGen, WorldData};
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_region::{DecoChunk, FLUID_TICK_NAMES, LiveOp, Region};
use rc_worldgen::structure::Start;
use rc_worldgen::template::{BBox, Mirror, Rot};

use super::src::{Fail, Src};
use super::{Parsed, R, Seg, args};
use crate::blocks::flag;
use crate::server::Server;
use crate::tr;

/// `RUSTCRAFT_OPT_DECO_PRIME` (default 1; `0`: one walk per map from the top of the world): the
/// height maps of a region copy (features of bone meal and commands, live structures) in a single
/// walk down each column, from the highest section that is not all air. Same heights.
fn deco_prime() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_DECO_PRIME", true)
}

/// `RUSTCRAFT_OPT_DECO_PAR` (default 0 until the quiet-machine bench; `1`: on): the region
/// copies of a live structure of 16 chunks or more shared among threads.
fn deco_par() -> bool {
    static C: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    crate::generation::opt_switch(&C, "RUSTCRAFT_OPT_DECO_PAR", false)
}

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
    /// Generation data and generator of the open dimension for `/place` and the live features
    /// (with the structures of the world).
    pub(crate) fn place_data(&mut self) -> Option<(Arc<WorldData>, Arc<DimGen>)> {
        if crate::generation::command_generators::one_command_generator() {
            let dg = self.command_generator(self.world_has_structures())?;
            return Some((self.world_data()?, dg));
        }
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
    pub(crate) fn structured_gen(&mut self) -> Option<(Arc<WorldData>, Arc<DimGen>)> {
        if crate::generation::command_generators::one_command_generator() {
            let dg = self.command_generator(true)?;
            return Some((self.world_data()?, dg));
        }
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
    /// chunk sees what its neighbours received; then the placements are replayed into the world in
    /// their order with their own flags (pool pieces: known shape, no shape update; built pieces:
    /// shape updates of the neighbours; neighbour updates of the templates without known shape),
    /// followed by the ticks, block entity data and entities. No post-processing pass: the live
    /// world's chunks drop the marks (game quirk). Randomness drawn from the world's.
    /// The game writes the pieces straight into the live world, which loads (generating it if
    /// needed) every chunk it touches: the chunks under the pieces are loaded first, and a journal
    /// that still reaches an unloaded chunk (feature of a pool element) loads it and starts the
    /// placement over on fresh copies, with the same draws.
    /// `reference`: position handed to the processors (`None`: the world generation's).
    pub(crate) fn place_start_live(
        &mut self,
        wd: &WorldData,
        dg: &DimGen,
        st: &Start,
        keep: bool,
        reference: Option<[i32; 3]>,
    ) -> bool {
        let (Some(sg), Some(deco)) = (dg.structures.clone(), dg.deco.clone()) else { return false };
        let mut under: Vec<(i32, i32)> = Vec::new();
        for p in &st.pieces {
            let b = p.bbox();
            for cz in (b.min[2] >> 4)..=(b.max[2] >> 4) {
                under.extend(((b.min[0] >> 4)..=(b.max[0] >> 4)).map(|cx| (cx, cz)));
            }
        }
        under.sort_unstable();
        under.dedup();
        self.load_now(&under);
        let hi = i64::from(self.bs.rng.next_int());
        let seed = (hi << 32).wrapping_add(i64::from(self.bs.rng.next_int()));
        let mut rounds = 0;
        let (grid, ops) = loop {
            let (grid, ops) = self.start_ops(wd, dg, &sg, &deco, st, seed, keep, reference);
            let mut reach: Vec<(i32, i32)> =
                ops.iter().map(|op| (op.pos()[0] >> 4, op.pos()[2] >> 4)).filter(|c| !self.chunk_present(*c)).collect();
            reach.sort_unstable();
            reach.dedup();
            rounds += 1;
            if reach.is_empty() || rounds == 3 {
                break (grid, ops);
            }
            self.load_now(&reach);
            if !reach.iter().any(|c| self.chunk_present(*c)) {
                break (grid, ops);
            }
        };
        self.replay_live(ops);
        self.deco_finish(&grid, &wd.blocks);
        true
    }

    /// Placement of a start on copies of the chunks of its box and their border (see
    /// [`Server::place_start_live`]): the copies and the journal of the placements.
    #[allow(clippy::too_many_arguments)]
    fn start_ops(
        &self,
        wd: &WorldData,
        dg: &DimGen,
        sg: &rc_worldgen::structure::StructureGen,
        deco: &rc_worldgen::decor::Decorator,
        st: &Start,
        seed: i64,
        keep: bool,
        reference: Option<[i32; 3]>,
    ) -> (Vec<DecoChunk>, Vec<LiveOp>) {
        let bb = st.bbox;
        let (c0, c1) = ((bb.min[0] >> 4, bb.min[2] >> 4), (bb.max[0] >> 4, bb.max[2] >> 4));
        let (gx0, gz0) = (c0.0 - 1, c0.1 - 1);
        let w = c1.0 - c0.0 + 3;
        let h = c1.1 - c0.1 + 3;
        let cells: Vec<(i32, i32)> = (0..h).flat_map(|dz| (0..w).map(move |dx| (gx0 + dx, gz0 + dz))).collect();
        let mut grid = self.copy_chunks(&cells, dg, wd);
        let mut rng = rc_worldgen::decor_rng::DecoRng::legacy(seed);
        let (min_y, top) = (self.dim.height.min_y, self.top_y());
        let idx = |x: i32, z: i32| ((z - gz0) * w + (x - gx0)) as usize;
        let mut ops: Vec<LiveOp> = Vec::new();
        // Chunks in the game's order (x varies first).
        for cz in c0.1..=c1.1 {
            for cx in c0.0..=c1.0 {
                let ids: [usize; 9] = std::array::from_fn(|i| idx(cx + i as i32 % 3 - 1, cz + i as i32 / 3 - 1));
                let Ok(refs) = grid.get_disjoint_mut(ids) else { continue };
                let mut r = Region::new(refs, &wd.blocks, dg.sea_level, dg.seed);
                r.live_log = Some(std::mem::take(&mut ops));
                let mut ctx = rc_worldgen::decor::Ctx::new(&mut r, deco);
                let zone = BBox::new([cx * 16, min_y, cz * 16], [cx * 16 + 15, top, cz * 16 + 15]);
                rc_worldgen::structure_place::place_start_zone(&mut ctx, sg, st, &mut rng, zone, keep, reference);
                ops = r.live_log.take().unwrap_or_default();
            }
        }
        (grid, ops)
    }

    /// Journal of a feature or a structure replayed into the live world in its order, each step
    /// with its own effects; the steps of an unloaded chunk are dropped. Returns the number of
    /// placements that changed a cell.
    pub(crate) fn replay_live(&mut self, ops: Vec<LiveOp>) -> usize {
        let mut n = 0;
        for op in ops {
            let p = op.pos();
            if !self.is_loaded(p) {
                continue;
            }
            match op {
                LiveOp::Set(_, s, f) => n += usize::from(self.set_block(p, s, f)),
                LiveOp::Notify(_) => {
                    let b = self.view().block(self.get(p));
                    self.update_neighbors_at(p, b);
                }
                LiveOp::Destroy(_) => {
                    self.destroy_block(p, true, 512);
                }
                LiveOp::Raw(_, s) => self.write_section_only(p, s),
            }
        }
        n
    }

    fn chunk_loaded_xz(&self, x: i32, z: i32) -> bool {
        self.chunk_present((x >> 4, z >> 4))
    }

    fn chunk_present(&self, c: (i32, i32)) -> bool {
        matches!(self.chunks.get(&c), Some(Some(_)))
    }

    /// Decoration chunk copied from a loaded chunk (blocks, biomes, heightmaps).
    pub(crate) fn deco_chunk(&self, cx: i32, cz: i32, dg: &DimGen, wd: &WorldData) -> DecoChunk {
        self.deco_chunk_with(cx, cz, dg, wd, deco_prime())
    }

    /// Region copies of `cells`, in order: shared among threads for a large grid when
    /// `RUSTCRAFT_OPT_DECO_PAR=1` (`RUSTCRAFT_THREADS`, otherwise the logical cores), one after
    /// the other otherwise.
    fn copy_chunks(&self, cells: &[(i32, i32)], dg: &DimGen, wd: &WorldData) -> Vec<DecoChunk> {
        let threads = if deco_par() && cells.len() >= 16 {
            rc_data::env_str!("RUSTCRAFT_THREADS")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
                .clamp(1, cells.len() / 4)
        } else {
            1
        };
        self.copy_chunks_on(cells, dg, wd, threads)
    }

    /// [`Server::copy_chunks`] on `threads` threads.
    pub(crate) fn copy_chunks_on(&self, cells: &[(i32, i32)], dg: &DimGen, wd: &WorldData, threads: usize) -> Vec<DecoChunk> {
        if threads <= 1 {
            return cells.iter().map(|&(cx, cz)| self.deco_chunk(cx, cz, dg, wd)).collect();
        }
        let (data, states, h, one_walk) = (&*self.data, &*self.states, self.dim.height, deco_prime());
        let live: Vec<Option<&crate::chunk::Chunk>> =
            cells.iter().map(|c| self.chunks.get(c).and_then(Option::as_deref)).collect();
        let per = cells.len().div_ceil(threads);
        std::thread::scope(|sc| {
            let parts: Vec<_> = cells
                .chunks(per)
                .zip(live.chunks(per))
                .map(|(cs, ls)| {
                    sc.spawn(move || {
                        cs.iter()
                            .zip(ls)
                            .map(|(&c, &l)| Self::copy_chunk(data, states, h, l, c, dg, wd, one_walk))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            parts.into_iter().flat_map(|p| p.join().expect("region copy thread")).collect()
        })
    }

    /// [`Server::deco_chunk`], height maps in one walk per column (`one_walk`) or one per map.
    pub(crate) fn deco_chunk_with(&self, cx: i32, cz: i32, dg: &DimGen, wd: &WorldData, one_walk: bool) -> DecoChunk {
        let live = self.chunks.get(&(cx, cz)).and_then(Option::as_deref);
        Self::copy_chunk(&self.data, &self.states, self.dim.height, live, (cx, cz), dg, wd, one_walk)
    }

    /// Region copy of a chunk (`live`: `None` for a chunk that is not loaded, all air); only
    /// shared data, so that copies can be made on several threads.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn copy_chunk(
        data: &rc_data::VersionData,
        states: &crate::states::States,
        h: crate::chunk::Height,
        live: Option<&crate::chunk::Chunk>,
        (cx, cz): (i32, i32),
        dg: &DimGen,
        wd: &WorldData,
        one_walk: bool,
    ) -> DecoChunk {
        let mut dc = DecoChunk::new(cx, cz, h.min_y, h.sections as i32 * 16);
        dc.blocks.fill(states.air_state);
        let conv = crate::generation_nbt::Conv::new(data, states, &wd.blocks, h, &dg.biome_names);
        if let Some(c) = live {
            for (si, sec) in c.sections.iter().enumerate() {
                sec.blocks.unpack_into(&mut dc.blocks[si * 4096..(si + 1) * 4096]);
                for i in 0..64 {
                    let reg = sec.biomes.get(i);
                    dc.biomes[si * 64 + i] = conv.reg_to_biome.get(&reg).copied().unwrap_or(0);
                }
            }
        }
        use rc_worldgen::decor_region::Hm;
        const MAPS: [Hm; 6] = [
            Hm::WorldSurfaceWg,
            Hm::WorldSurface,
            Hm::OceanFloorWg,
            Hm::OceanFloor,
            Hm::MotionBlocking,
            Hm::MotionBlockingNoLeaves,
        ];
        if one_walk {
            // Top of the highest section that is not all air: no map counts what is above.
            let top = live
                .and_then(|c| {
                    c.sections.iter().rposition(|s| !matches!(s.blocks, rc_world::Container::Single(v) if states.is_air(v)))
                })
                .map_or(h.min_y, |si| h.min_y + (si as i32 + 1) * 16);
            dc.prime_many(&wd.blocks, &MAPS, top);
        } else {
            for hm in MAPS {
                dc.prime(&wd.blocks, hm);
            }
        }
        dc
    }

    /// What a feature or a structure left in the loaded chunks of its region besides its
    /// placements (already replayed): ticks, block entity data (bees of a hive, loot tables,
    /// templates), then entities.
    pub(crate) fn deco_finish(&mut self, chunks: &[DecoChunk], b: &Blocks) {
        for dc in chunks {
            if !self.chunk_present((dc.cx, dc.cz)) {
                continue;
            }
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
        for dc in chunks {
            if !self.chunk_present((dc.cx, dc.cz)) {
                continue;
            }
            for (p, d) in &dc.be_data {
                if self.be_get(*p).is_some() {
                    let mut f = Vec::new();
                    crate::generation_nbt::be_extra_fields(d, &mut f);
                    self.set_block_data(*p, Nbt::Compound(f));
                }
            }
            if dc.entities.is_empty() {
                continue;
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
        let Some(deco) = self.place_data().and_then(|(_, dg)| dg.deco.clone()) else {
            return Err(Fail(tr!("commands.place.feature.failed")));
        };
        if deco.reg.feature_id(&id).is_none() {
            return Err(Fail(tr!("argument.resource.not_found", id, "minecraft:worldgen/configured_feature")));
        }
        // Same placement as the bone meal features (world's random source, journal replayed).
        if !self.place_world_feature(&id, pos) {
            return Err(Fail(tr!("commands.place.feature.failed")));
        }
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
        self.place_start_live(&wd, &dg, &st, false, None);
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
        Some(self.place_start_live(&wd, &dg, &st, keep, Some(pos)))
    }
}
