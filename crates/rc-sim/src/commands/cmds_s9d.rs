//! S9: `place feature` (configured features of the generator, `rc-worldgen`) and `place template`
//! (`.nbt` templates from the jar or the world); `place structure` and `place jigsaw` are not
//! yet wired to structure generation (clean failure).

use std::sync::Arc;

use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::{DimGen, WorldData};
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_region::{DecoChunk, FLUID_TICK_NAMES, Region};
use rc_worldgen::template::{Mirror, Rot};

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
        self.prog.place = Some(Box::new(PlaceData { dim: self.dim.name.clone(), wd: wd.clone(), dg: dg.clone() }));
        Some((wd, dg))
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
                for i in 0..4096 {
                    dc.blocks[si * 4096 + i] = sec.blocks.get(i);
                }
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

    /// Writes the changed blocks of a decoration chunk into the world ("clients" flag).
    pub(crate) fn apply_deco(&mut self, dc: &DecoChunk, b: &Blocks) -> usize {
        let h = self.dim.height;
        let mut n = 0;
        let Some(Some(_)) = self.chunks.get(&(dc.cx, dc.cz)) else { return 0 };
        for si in 0..h.sections {
            for i in 0..4096 {
                let new = dc.blocks[si * 4096 + i];
                let (x, z, y) = ((i & 15) as i32, ((i >> 4) & 15) as i32, (i >> 8) as i32);
                let p = [dc.cx * 16 + x, h.min_y + si as i32 * 16 + y, dc.cz * 16 + z];
                if self.get(p) != new {
                    self.set_block(p, new, flag::CLIENTS);
                    n += 1;
                }
            }
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
        n
    }

    pub(crate) fn place_feature_cmd(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let id = args::id(&p.arg(seg, "feature").unwrap_or_default());
        let pos = match p.arg(seg, "pos") {
            Some(t) => args::block_pos(self, s, &t),
            None => crate::mobs::mth::block_of(s.pos),
        };
        if !self.chunk_loaded_xz(pos[0], pos[2]) {
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
        let placed = {
            let mut it = chunks.iter_mut();
            let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().expect("9 chunks"));
            let mut r = Region::new(refs, &wd.blocks, dg.sea_level, dg.seed);
            let mut ctx = rc_worldgen::decor::Ctx::new(&mut r, &deco);
            let mut rng = rc_worldgen::decor_rng::DecoRng::legacy(crate::rng::Rng::from_time().next_long());
            rc_worldgen::feature::place_feature(&mut ctx, fid, &mut rng, pos)
        };
        if !placed {
            return Err(Fail(tr!("commands.place.feature.failed")));
        }
        for dc in &chunks {
            self.apply_deco(dc, &wd.blocks);
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
        let Some((wd, _)) = self.place_data() else { return Err(Fail(tr!("commands.place.template.invalid", id))) };
        // Template of the world (`generated/<ns>/structures/<path>.nbt`), otherwise of the jar.
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id.as_str()));
        let world_file = self.world.join("generated").join(ns).join("structures").join(format!("{path}.nbt"));
        let tpl = match crate::nbtio::read_dat(&world_file) {
            Ok(root) => rc_worldgen::template::Template::parse(&root, &wd.blocks).ok().map(Arc::new),
            Err(_) => wd.templates.get(&id, &wd.blocks),
        };
        let Some(t) = tpl else { return Err(Fail(tr!("commands.place.template.invalid", id))) };
        let far = [pos[0] + t.size[0], pos[1] + t.size[1], pos[2] + t.size[2]];
        for cx in (pos[0].min(far[0]) >> 4)..=(pos[0].max(far[0]) >> 4) {
            for cz in (pos[2].min(far[2]) >> 4)..=(pos[2].max(far[2]) >> 4) {
                if !matches!(self.chunks.get(&(cx, cz)), Some(Some(_))) {
                    return Err(Fail(tr!("argument.pos.unloaded")));
                }
            }
        }
        let Some(palette) = t.palettes.first() else { return Err(Fail(tr!("commands.place.template.failed"))) };
        if palette.is_empty() && t.entities.is_empty() {
            return Err(Fail(tr!("commands.place.template.failed")));
        }
        // Integrity: each block kept with the given probability (command seed).
        let mut rng =
            crate::rng::Rng::new(if seed == 0 { crate::rng::Rng::from_time().next_long() } else { i64::from(seed) });
        let flags = flag::CLIENTS | if strict { 816 } else { 0 };
        for b in palette {
            if integrity < 1.0 && rng.next_float() > integrity {
                continue;
            }
            let rp = rc_worldgen::template::transform(b.pos, mirror, rot, [0, 0, 0]);
            let wp = [pos[0] + rp[0], pos[1] + rp[1], pos[2] + rp[2]];
            let st = wd.templates.transforms.apply(b.state, mirror, rot);
            self.set_block(wp, st, flags);
            if let Some(n) = &b.nbt {
                let mut be = n.clone();
                crate::nbtio::set(&mut be, "x", Nbt::Int(wp[0]));
                crate::nbtio::set(&mut be, "y", Nbt::Int(wp[1]));
                crate::nbtio::set(&mut be, "z", Nbt::Int(wp[2]));
                self.set_block_data(wp, be);
            }
        }
        self.ok(s, tr!("commands.place.template.success", id, pos[0], pos[1], pos[2]));
        Ok(1)
    }

    pub(crate) fn place_structure_cmd(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let _ = (s, p, seg);
        Err(Fail(tr!("commands.place.structure.failed")))
    }

    pub(crate) fn place_jigsaw_cmd(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let _ = (s, p, seg);
        Err(Fail(tr!("commands.place.jigsaw.failed")))
    }
}
