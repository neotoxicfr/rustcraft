//! From a block change to a mesh job: light recompute of the touched sections, then scheduling of the meshing.

use rc_mesh::blocks::{self, Tables};
use rc_mesh::wire::MeshJob;
use std::time::Instant;

use crate::{App, environ, light_engine, perf_log, remesh, weather};

/// Where a block change applied to the client world comes from: decides the local relight,
/// and splits the changes that touch the light in the perf log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeOrigin {
    /// Sent by the integrated server right after the light of its section: no local relight
    /// ([`light_engine::Opts::solo_cut`]).
    ServerLightReceived,
    /// Sent by the server, without fresh light for its section: local relight.
    Server,
    /// Piston replayed when its block event arrives.
    PistonEvent,
    /// Moving block placed at the end of its motion.
    PistonFinish,
    /// The player's own action predicted, or the prediction rolled back.
    Prediction,
}

impl ChangeOrigin {
    pub(crate) const ALL: [ChangeOrigin; 5] = [
        ChangeOrigin::ServerLightReceived,
        ChangeOrigin::Server,
        ChangeOrigin::PistonEvent,
        ChangeOrigin::PistonFinish,
        ChangeOrigin::Prediction,
    ];

    /// The client recomputes the light of the change itself.
    pub(crate) fn relights(self) -> bool {
        self != ChangeOrigin::ServerLightReceived
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            ChangeOrigin::ServerLightReceived => "server_light_received",
            ChangeOrigin::Server => "server",
            ChangeOrigin::PistonEvent => "piston_event",
            ChangeOrigin::PistonFinish => "piston_finish",
            ChangeOrigin::Prediction => "prediction",
        }
    }
}

/// Changes of section `[cx, sy, cz]` (`sy` from the bottom of the world) that touch the light:
/// world position (y from the bottom of the world) and whether they touch the sky too. `old`: state
/// of a cell before the change.
fn light_touching_changes<'a>(
    changes: &'a [(u16, u32)],
    old: impl Fn(usize) -> u32 + 'a,
    light: &'a light_engine::LightTables,
    [cx, sy, cz]: [i32; 3],
) -> impl Iterator<Item = ([i32; 3], bool)> + 'a {
    changes.iter().filter(|&&(i, _)| usize::from(i) < rc_world::SECTION_BLOCKS).filter_map(move |&(i, state)| {
        let before = old(usize::from(i));
        if !light.matters(before, state) {
            return None;
        }
        let i = i32::from(i);
        let p = [cx * 16 + (i & 15), sy * 16 + (i >> 8), cz * 16 + ((i >> 4) & 15)];
        Some((p, light.matters_sky(before, state)))
    })
}

impl App {
    /// Directional shading of the dimension (cardinal lighting), for the mesher and the shader.
    pub(crate) fn set_shade(&mut self, nether: bool) {
        self.shade = if nether { rc_mesh::wire::SHADE_NETHER } else { rc_mesh::wire::SHADE_DEFAULT };
        if let Some(r) = &mut self.renderer {
            r.shade = self.shade;
        }
    }

    /// Change predicted by the client (the player's action or its rollback): local light
    /// recompute always.
    pub(crate) fn update_section(
        &mut self,
        cx: i32,
        cz: i32,
        sy: i32,
        changes: &[(u16, u32)],
        air: impl Fn(u32, &Tables) -> bool,
    ) {
        self.update_section_lit(cx, cz, sy, changes, air, ChangeOrigin::Prediction);
    }

    /// Change received from the server: local light recompute unless the integrated server has just
    /// sent the section's ([`light_engine::Opts::solo_cut`]).
    pub(crate) fn server_origin(&self, cx: i32, sy: i32, cz: i32) -> ChangeOrigin {
        if light_engine::opts().solo_cut && self.solo.is_some() && self.light_recv.covers(cx, sy, cz) {
            ChangeOrigin::ServerLightReceived
        } else {
            ChangeOrigin::Server
        }
    }

    pub(crate) fn update_section_lit(
        &mut self,
        cx: i32,
        cz: i32,
        sy: i32,
        changes: &[(u16, u32)],
        air: impl Fn(u32, &Tables) -> bool,
        origin: ChangeOrigin,
    ) {
        let bits = self.world.state_bits;
        let tables = self.tables.clone();
        let Some(col) = self.world.grid.get_mut_by(cx, cz, rc_world::ColumnWriter::Blocks) else { return };
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
        // Changes that touch the light: local relight before the next meshing. With the perf log,
        // those the integrated server has just lit are also looked at, to be counted.
        let (relight, counted) = (origin.relights(), perf_log::on());
        if relight || counted {
            let mut found = 0;
            for (p, sky) in light_touching_changes(changes, |i| sec.blocks.get(i), &self.light, [cx, sy, cz]) {
                found += 1;
                if relight {
                    self.relight.push(p);
                    if sky {
                        self.relight_sky.push(p);
                    }
                }
            }
            if counted {
                perf_log::light_changes(origin, found);
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
        self.world.meshes.tick(Instant::now());
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
            let live = !mask.is_all();
            let kind = match (live, &reuse) {
                (false, _) => perf_log::MeshJobKind::Whole,
                (true, Some(_)) => perf_log::MeshJobKind::PartialWithBase,
                (true, None) => perf_log::MeshJobKind::PartialWithoutBase,
            };
            job.reuse = reuse;
            if live && perf_log::on() {
                self.count_partial_reach(x, z);
            }
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
                perf_log::mesh_work(t.elapsed(), secs, kind);
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

    /// Perf log: partial remesh of a column the last visibility walk reached or not (the game
    /// only rebuilds the sections it sees); nothing counted before the first walk.
    fn count_partial_reach(&self, x: i32, z: i32) {
        if let Some(reached) = self.renderer.as_ref().and_then(|r| r.column_reached(x, z)) {
            perf_log::partial_reach(reached);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a change sent right after the integrated server's light skips the local relight (the
    /// boolean of before); one name per origin; `ALL` in discriminant order (the perf log indexes
    /// its counters by it).
    #[test]
    fn origins_relight_like_before() {
        for (k, o) in ChangeOrigin::ALL.iter().enumerate() {
            assert_eq!(o.relights(), *o != ChangeOrigin::ServerLightReceived, "{o:?}");
            assert_eq!(*o as usize, k, "{o:?}");
        }
        let mut names: Vec<_> = ChangeOrigin::ALL.iter().map(|o| o.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ChangeOrigin::ALL.len());
    }

    /// Changes that touch the light (26.3 tables): torch on air (block light only), stone on air
    /// (sky too), air on air and an index outside the section ignored; world positions as before.
    #[test]
    fn light_touching_changes_keep_the_old_positions() {
        let Some(d) = rc_testdata::version_data() else {
            eprintln!("26.3 data absent: test ignored");
            return;
        };
        let t = light_engine::LightTables::new(&d);
        let state = |name: &str| {
            let short = name.trim_start_matches("minecraft:");
            d.blocks.iter().find(|b| b.name == name || b.name == short).map(|b| b.default_state).expect("block")
        };
        let (torch, stone) = (state("minecraft:torch"), state("minecraft:stone"));
        let cell = |x: u16, y: u16, z: u16| (y << 8) | (z << 4) | x;
        let changes = [(cell(5, 7, 3), torch), (cell(0, 15, 15), stone), (cell(1, 1, 1), 0), (4096, stone)];
        let found: Vec<_> = light_touching_changes(&changes, |_| 0, &t, [2, 3, -1]).collect();
        assert_eq!(found, [([37, 55, -13], false), ([32, 63, -1], true)]);
    }
}
