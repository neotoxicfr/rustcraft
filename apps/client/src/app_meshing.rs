//! From a block change to a mesh job: light recompute of the touched sections, then scheduling of the meshing.

use rc_mesh::blocks::{self, Tables};
use rc_mesh::wire::MeshJob;
use std::time::Instant;

use crate::{App, environ, light_engine, perf_log, remesh, weather};

impl App {
    /// Directional shading of the dimension (cardinal lighting), for the mesher and the shader.
    pub(crate) fn set_shade(&mut self, nether: bool) {
        self.shade = if nether { rc_mesh::wire::SHADE_NETHER } else { rc_mesh::wire::SHADE_DEFAULT };
        if let Some(r) = &mut self.renderer {
            r.shade = self.shade;
        }
    }

    /// Change made by the client itself (prediction, acknowledgement, piston replay): local
    /// light recompute always.
    pub(crate) fn update_section(
        &mut self,
        cx: i32,
        cz: i32,
        sy: i32,
        changes: &[(u16, u32)],
        air: impl Fn(u32, &Tables) -> bool,
    ) {
        self.update_section_lit(cx, cz, sy, changes, air, true);
    }

    /// Change received from the server: local light recompute unless the integrated server has just
    /// sent the section's ([`light_engine::Opts::solo_cut`]).
    pub(crate) fn server_relight(&self, cx: i32, sy: i32, cz: i32) -> bool {
        !(light_engine::opts().solo_cut && self.solo.is_some() && self.light_recv.covers(cx, sy, cz))
    }

    pub(crate) fn update_section_lit(
        &mut self,
        cx: i32,
        cz: i32,
        sy: i32,
        changes: &[(u16, u32)],
        air: impl Fn(u32, &Tables) -> bool,
        relight: bool,
    ) {
        let bits = self.world.state_bits;
        let tables = self.tables.clone();
        let Some(col) = self.world.grid.get_mut(cx, cz) else { return };
        let Some(sec) = usize::try_from(sy).ok().and_then(|i| col.sections.get_mut(i)) else { return };
        // Block entities: removal, new state or creation (`block-entities.md` § 1.1).
        for &(i, state) in changes {
            if usize::from(i) < rc_world::SECTION_BLOCKS {
                let i32i = i32::from(i);
                let pos =
                    [cx * 16 + (i32i & 15), self.world.min_y + sy * 16 + (i32i >> 8), cz * 16 + ((i32i >> 4) & 15)];
                let old = sec.blocks.get(usize::from(i));
                self.block_entities.on_block_change(pos, old, state);
                let block = |s: u32| self.data.states.get(s as usize).map(|s| s.block);
                self.tech.on_block_change(pos, block(old) == block(state));
            }
        }
        // Changes that touch the light: local relight before the next meshing.
        for &(i, state) in changes.iter().filter(|&&(i, _)| relight && usize::from(i) < rc_world::SECTION_BLOCKS) {
            let old = sec.blocks.get(usize::from(i));
            if self.light.matters(old, state) {
                let i = i32::from(i);
                let p = [cx * 16 + (i & 15), sy * 16 + (i >> 8), cz * 16 + ((i >> 4) & 15)];
                self.relight.push(p);
                if self.light.matters_sky(old, state) {
                    self.relight_sky.push(p);
                }
            }
        }
        // Cells whose state really changes: only their sections and neighbours are remeshed.
        let changed: Vec<u16> = changes
            .iter()
            .filter(|&&(i, s)| usize::from(i) < rc_world::SECTION_BLOCKS && sec.blocks.get(usize::from(i)) != s)
            .map(|&(i, _)| i)
            .collect();
        sec.set_blocks(changes, bits, |s| air(s, &tables));
        // Client height maps, after the section (cells really changed only).
        let height = self.world.height as usize;
        for &i in &changed {
            let i = usize::from(i);
            let state = col.sections[sy as usize].blocks.get(i);
            col.update_heightmaps(i & 15, sy as usize * 16 + (i >> 8), (i >> 4) & 15, state, &self.world.hm_rules, height);
        }
        self.world.touch_blocks(cx, sy, cz, changed.into_iter());
        let view = environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let b = weather::Blocking { data: &self.data, phys: &self.phys };
        self.weather.on_blocks(&view, [cx * 16, sy * 16 + self.world.min_y, cz * 16], changes, &b);
    }

    /// Starts the meshing of the modified columns (at most four tasks per pool thread,
    /// `RUSTCRAFT_MESHQUEUE`: a deep queue keeps the pool busy during the long frames of
    /// loading; bench of 19/10 in `docs/perf.md`).
    pub(crate) fn schedule_meshing(&mut self) {
        static DEPTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let depth =
            *DEPTH.get_or_init(|| rc_data::env_str!("RUSTCRAFT_MESHQUEUE").and_then(|v| v.parse().ok()).unwrap_or(4));
        let limit = rayon::current_num_threads() * depth;
        let free = limit.saturating_sub(self.in_flight);
        if free == 0 || self.world.dirty.is_empty() {
            return;
        }
        // The closest to the camera first: the world appears around the player.
        let (ex, ez) = ((self.eye[0] / 16.0).floor() as i32, (self.eye[2] / 16.0).floor() as i32);
        // A column already in progress waits for its result (base of the next remesh), unless it
        // must be redone whole.
        let meshes = &self.world.meshes;
        let mut ready: Vec<(i32, i32)> =
            self.world.dirty.iter().filter(|&(&k, &m)| meshes.may_start(k, m)).map(|(&k, _)| k).collect();
        let dist = |&k: &(i32, i32)| remesh::column_dist2(k, (ex, ez));
        if ready.len() > free {
            ready.select_nth_unstable_by_key(free - 1, dist);
            ready.truncate(free);
        }
        ready.sort_unstable_by_key(dist);
        for (x, z) in ready {
            let Some(mask) = self.world.dirty.remove(&(x, z)) else { continue };
            let Some(center) = self.world.grid.get_arc(x, z) else { continue };
            let version = self.world.versions.get(&(x, z)).copied().unwrap_or(0);
            let mut job = MeshJob::gather(center, version, |x, z| self.world.grid.get_arc(x, z));
            let (reuse, base_version) = match self.world.mesh_reuse((x, z), mask, version) {
                Some((s, (w, v))) => (Some((s, w)), Some(v)),
                None => (None, None),
            };
            job.reuse = reuse;
            let live = !mask.is_all();
            self.world.meshes.note_job(&job);
            job.shade = self.shade;
            job.has_sky = self.env.has_sky;
            job.opts = self.video.mesh;
            job.plains_biome = self.env.plains();
            (job.biome_zoom_seed, job.min_y) = (crate::environ::biome_zoom_seed(), self.world.min_y);
            let (tables, tx, min_y, palette) =
                (self.tables.clone(), self.meshes_tx.clone(), self.world.min_y, self.palette.clone());
            self.in_flight += 1;
            let n = job.center.sections.len();
            let secs = job.reuse.as_ref().map_or(n, |(m, _)| m.count(n));
            // Priority updates (`prioritizeChunkUpdates`): in-place meshing, visible in
            // this frame (never delayed by the upload budget).
            let urgent = self.video.mesh_sync(x, z, self.eye, self.options.i("prioritizeChunkUpdates"));
            let run = move || {
                let t = Instant::now();
                let m = blocks::mesh(&job, &tables, &palette, min_y);
                perf_log::mesh_work(t.elapsed(), secs);
                let t = Instant::now();
                let base = job.reuse.as_ref().zip(base_version).map(|((_, w), v)| (w, v));
                let done = remesh::finish(m, live, urgent, base);
                perf_log::prep_work(t.elapsed());
                let _ = tx.send(done);
            };
            if urgent {
                run();
                continue;
            }
            rayon::spawn(run);
        }
    }
}
