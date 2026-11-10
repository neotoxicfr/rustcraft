//! From a block change to a mesh job: light recompute of the touched sections, then scheduling of the meshing.

use hashbrown::HashMap;
use rc_mesh::blocks::{self, BiomePalette, Tables};
use rc_mesh::wire::{MeshJob, Sections};
use std::sync::Arc;
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
    /// loading; bench of 19/10 in `docs/perf.md`). Only their dirty sections in view, like the
    /// game ([`sections_to_launch`]), and only the columns some view can launch are tested against
    /// the view ([`MeshCandidates`]).
    pub(crate) fn schedule_meshing(&mut self) {
        static DEPTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        let depth =
            *DEPTH.get_or_init(|| rc_data::env_str!("RUSTCRAFT_MESHQUEUE").and_then(|v| v.parse().ok()).unwrap_or(4));
        self.world.meshes.tick(Instant::now());
        let limit = rayon::current_num_threads() * depth;
        let free = limit.saturating_sub(self.in_flight);
        if !pick_this_frame(free, self.world.dirty.len(), perf_log::on()) {
            return;
        }
        // The closest to the camera first: the world appears around the player.
        let (ex, ez) = ((self.eye[0] / 16.0).floor() as i32, (self.eye[2] / 16.0).floor() as i32);
        let (eye, sync_mode) = (self.eye, self.options.i("prioritizeChunkUpdates"));
        let visible_only = mesh_visible_only();
        // Every dirty column was held last time and nothing they depend on changed since: they
        // would all be held again, so the pick is skipped (it tests each one against the view).
        let signature = self.renderer.as_ref().filter(|_| visible_only).map(|r| HeldSignature {
            view: r.meshing_view_stamp(),
            marks: self.world.counter,
            dirty: self.world.dirty.len(),
            in_flight: self.in_flight,
            sync_mode,
        });
        if signature.is_some() && signature == self.all_held && skip_unchanged_held() {
            return;
        }
        let (renderer, video) = (self.renderer.as_ref(), &self.video);
        // Only the columns the view can launch are tested against it.
        let listed = renderer.filter(|_| visible_only && pick_reached_marks_only()).and_then(|r| {
            candidates_of_frame(&mut self.mesh_candidates, &self.world, r, video, (ex, ez), sync_mode)
        });
        let meshes = &self.world.meshes;
        // The gauge leaves out the columns in progress, listed or not (only counted with the perf
        // log: it walks the tasks in flight).
        let among = listed.map(|columns| Among {
            columns,
            dirty_in_progress: if perf_log::on() { meshes.dirty_in_progress(&self.world.dirty) } else { 0 },
        });
        // Like the game, a column already drawn is remeshed only for its dirty sections in view.
        let launchable = |(x, z): (i32, i32), marks: Sections| {
            let Some(r) = renderer.filter(|_| visible_only) else { return Some(marks) };
            let urgent = video.wants_sync(x, z, eye, sync_mode);
            sections_to_launch(marks, r.sections_in_view(x, z), meshes.has_base((x, z)), urgent)
        };
        // A column already in progress waits for its result (base of the next remesh), unless it
        // must be redone whole.
        let picked = pick_columns(
            &mut self.world.dirty,
            among,
            free,
            (ex, ez),
            |k, m| meshes.may_start(k, m),
            launchable,
        );
        if perf_log::on() {
            perf_log::mesh_held(picked.held, picked.partly_launched);
        }
        self.all_held = signature.filter(|_| picked.launches.is_empty() && !self.world.dirty.is_empty());
        let release_early = release_job_early();
        for ((x, z), mask) in picked.launches {
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
            let tx = self.meshes_tx.clone();
            self.in_flight += 1;
            let n = job.center.sections.len();
            let sections = job.reuse.as_ref().map_or(n, |(m, _)| m.count(n));
            // Priority updates (`prioritizeChunkUpdates`): in-place meshing, visible in
            // this frame (never delayed by the upload budget).
            let urgent = self.video.mesh_sync(x, z, self.eye, sync_mode);
            let task = MeshTask {
                job,
                tables: self.tables.clone(),
                palette: self.palette.clone(),
                min_y: self.world.min_y,
                live,
                urgent,
                base_version,
                kind,
                sections,
            };
            let run = move || {
                let (done, kept_job) = task.run(release_early);
                let _ = tx.send(done);
                // Old path: the job's columns were held until the result was sent.
                drop(kept_job);
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

/// The candidates of this frame's pick ([`MeshCandidates::list`]), `None` when the pick tests every
/// dirty column. `sync_mode`: the priority option (`prioritizeChunkUpdates`).
fn candidates_of_frame<'a>(
    candidates: &'a mut MeshCandidates,
    world: &crate::World,
    renderer: &crate::Renderer,
    video: &crate::video::VideoState,
    eye_column: (i32, i32),
    sync_mode: i32,
) -> Option<&'a [(i32, i32)]> {
    let key =
        candidate_key(world.counter, renderer.meshing_reach_stamp(), sync_mode, video.player_change_marks(), eye_column);
    candidates.list(key, &world.dirty, |(x, z), marks| {
        let urgent = listed_as_priority(video.changed_by_player(x, z), (x, z), eye_column, sync_mode);
        view_may_launch(marks, renderer.reached_sections(x, z), urgent)
    })
}

/// The candidate list's key ([`CandidateKey`]): the eye's column counts only for the nearby
/// priority remesh (option 2), the only rule that reads it.
pub(crate) fn candidate_key(
    marks: u64,
    reach: ReachStamp,
    sync_mode: i32,
    player_marks: usize,
    eye_column: (i32, i32),
) -> CandidateKey {
    CandidateKey { marks, reach, sync_mode, player_marks, eye_column: (sync_mode == 2).then_some(eye_column) }
}

/// A dirty column listed whatever the walk because a priority remesh may take it: one the player
/// changed (options 1 and 2), and for the nearby priority (option 2) every column it may reach while
/// the eye stays in its column. It covers every column the pick's priority test answers yes for.
pub(crate) fn listed_as_priority(changed_by_player: bool, key: (i32, i32), eye_column: (i32, i32), sync_mode: i32) -> bool {
    (sync_mode >= 1 && changed_by_player) || (sync_mode == 2 && near_eye_column(key, eye_column))
}

/// Visibility filter of the meshing (`RUSTCRAFT_OPT_MESHVISIBLEONLY`, on by default; `=0`: every
/// dirty column as before): the game rebuilds only the dirty sections in view, the others keep
/// their mark until they come into view (`docs/specs/visibility.md` § 3).
fn mesh_visible_only() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_MESHVISIBLEONLY")
}

/// Inputs of the visibility filter's verdicts: the renderer's meshing view stamp, the marks (the
/// world's touch counter and the dirty columns), the tasks in flight (a column in progress is not
/// picked until its result arrives) and the priority option. Two equal signatures give the same
/// held columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldSignature {
    view: [u64; 4],
    marks: u64,
    dirty: usize,
    in_flight: usize,
    sync_mode: i32,
}

/// The pick is skipped while every dirty column stays held for the same reasons
/// (`RUSTCRAFT_OPT_MESH_SKIP_UNCHANGED_HELD`, on by default; `=0`: every frame tests each held
/// column against the view again). Bench of 07/11: on plains32 the columns held out of view took
/// 0.11 ms per frame, 2,555 instead of 3,563 frames per second.
fn skip_unchanged_held() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_MESH_SKIP_UNCHANGED_HELD")
}

/// The pick tests against the view only the dirty columns it can launch ([`MeshCandidates`],
/// `RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS`, on by default; `=0`: every dirty column each pick).
fn pick_reached_marks_only() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS")
}

/// A dirty column some view can launch: never drawn (`reached` is `None`), a priority remesh, or
/// marks on a section the walk reached. The game rebuilds only the dirty sections of its list of
/// sections in view, which the walk fills: a mark outside the reached sections is held under
/// every view until the walk changes.
pub(crate) fn view_may_launch(marks: Sections, reached: Option<Sections>, urgent: bool) -> bool {
    urgent || reached.is_none_or(|reached| !marks.intersection(reached).is_empty())
}

/// Column `key` lies within two columns of the eye's: every column whose centre is within √768
/// blocks of the eye does (the nearby priority remesh, `prioritizeChunkUpdates` 2), wherever the
/// eye stands in its column.
fn near_eye_column(key: (i32, i32), eye_column: (i32, i32)) -> bool {
    (key.0 - eye_column.0).abs() <= 2 && (key.1 - eye_column.1).abs() <= 2
}

/// Everything the candidate list depends on: the world's touch counter (every new mark bumps it),
/// the renderer's reach stamp (walk, columns drawn), the priority option, the columns the player
/// changed (priority remesh) and, for the nearby priority remesh, the eye's column.
/// Made by [`candidate_key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CandidateKey {
    marks: u64,
    reach: ReachStamp,
    sync_mode: i32,
    player_marks: usize,
    eye_column: Option<(i32, i32)>,
}

/// What the renderer's reached sections depend on (`Renderer::meshing_reach_stamp`). The view is
/// left out: it changes every frame while turning, the reached sections do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReachStamp {
    /// A frame was drawn (before, no column counts as reached).
    pub frame_drawn: bool,
    /// Generation of the walk's reached sections.
    pub walk_generation: u64,
    /// Connectivity revision: bumped when a column appears or leaves.
    pub connectivity_revision: u64,
}

/// Dirty columns the pick tests against the view ([`view_may_launch`]); every other dirty column is
/// held whatever the view, so a turning camera costs the columns that can show, not every column
/// waiting with marks the walk does not reach. The list is made again when its key changes; a
/// column launched meanwhile stays listed (the pick skips it once it leaves the dirty columns).
#[derive(Default)]
pub(crate) struct MeshCandidates {
    key: Option<CandidateKey>,
    /// Key of the previous pick (the list is made on the second pick in a row with one key).
    seen: Option<CandidateKey>,
    pub columns: Vec<(i32, i32)>,
}

impl MeshCandidates {
    /// The list for `key`, made from `dirty` on the second pick in a row with this key; `None`
    /// before (the pick tests every dirty column). Making the list costs about what testing every
    /// column does, so while the key changes every frame (new marks every frame when flying) a list
    /// that would serve once is not made; while it repeats (turning in place) it serves every frame.
    pub(crate) fn list(
        &mut self,
        key: CandidateKey,
        dirty: &HashMap<(i32, i32), Sections>,
        may_launch: impl Fn((i32, i32), Sections) -> bool,
    ) -> Option<&[(i32, i32)]> {
        if self.key != Some(key) && self.seen.replace(key) != Some(key) {
            return None;
        }
        self.refresh(key, dirty, may_launch);
        Some(&self.columns)
    }

    /// Makes the list again from `dirty` when `key` differs from the one it was made for.
    pub(crate) fn refresh(
        &mut self,
        key: CandidateKey,
        dirty: &HashMap<(i32, i32), Sections>,
        may_launch: impl Fn((i32, i32), Sections) -> bool,
    ) {
        if self.key == Some(key) {
            return;
        }
        self.columns.clear();
        self.columns.extend(dirty.iter().filter(|&(&k, &m)| may_launch(k, m)).map(|(&k, _)| k));
        self.key = Some(key);
    }
}

/// The task lets go of its nine columns as soon as the mesh is made
/// (`RUSTCRAFT_OPT_MESHDROPJOBEARLY`, on by default; `=0`: held until the result is sent).
fn release_job_early() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_MESHDROPJOBEARLY")
}

/// Sections a dirty column launches under the visibility filter, `None` to hold it this frame.
/// `in_view`: its sections reached by the walk and kept by the field of the last frame drawn;
/// `None` when the walk cannot see the column (no mesh of it drawn yet): launched as before, or
/// the walk would never reach it. A priority (synchronous) remesh is never held. Without a kept
/// base, or redone whole, the task meshes the whole column anyway: every mark goes as soon as one
/// dirty section is in view.
pub(crate) fn sections_to_launch(
    dirty: Sections,
    in_view: Option<Sections>,
    has_base: bool,
    urgent: bool,
) -> Option<Sections> {
    let Some(in_view) = in_view.filter(|_| !urgent) else { return Some(dirty) };
    let seen = dirty.intersection(in_view);
    if seen.is_empty() {
        None
    } else if dirty.is_all() || !has_base {
        Some(dirty)
    } else {
        Some(seen)
    }
}

/// The frame runs [`pick_columns`]: something to launch (`free` task slots, `dirty` columns), or the
/// perf log on (`counting_held`), whose held-columns gauge must stay current even with a full queue
/// or nothing dirty (the pick then launches nothing and leaves the marks as they are).
fn pick_this_frame(free: usize, dirty: usize, counting_held: bool) -> bool {
    counting_held || (free > 0 && dirty > 0)
}

/// A dirty column and the sections it launches.
type Launch = ((i32, i32), Sections);

/// The only dirty columns the pick tests ([`MeshCandidates`], each listed once), and for the
/// held-columns gauge the number of dirty columns in progress, listed or not (`0` without the perf
/// log).
#[derive(Clone, Copy)]
pub(crate) struct Among<'a> {
    pub columns: &'a [(i32, i32)],
    pub dirty_in_progress: usize,
}

/// What [`pick_columns`] gives: the launches, and for the perf log the columns held (startable and
/// not launched) and those launched in part.
pub(crate) struct Picked {
    pub launches: Vec<Launch>,
    pub held: usize,
    pub partly_launched: usize,
}

/// Columns to mesh this frame with the sections of each: at most `free`, the closest to the eye
/// column first. `launchable` tells what a startable column launches (`None`: held); what is
/// launched leaves `dirty`, the sections held stay marked. `among`: the only dirty columns that
/// can launch, the others held without a test; `None`: every dirty column.
pub(crate) fn pick_columns(
    dirty: &mut HashMap<(i32, i32), Sections>,
    among: Option<Among<'_>>,
    free: usize,
    eye: (i32, i32),
    may_start: impl Fn((i32, i32), Sections) -> bool,
    launchable: impl Fn((i32, i32), Sections) -> Option<Sections>,
) -> Picked {
    let (mut ready, mut held, mut in_progress) = (Vec::new(), 0, 0);
    let mut classify = |key: (i32, i32), marks: Sections| {
        if !may_start(key, marks) {
            in_progress += 1;
            return;
        }
        match launchable(key, marks) {
            Some(launch) => ready.push((key, launch)),
            None => held += 1,
        }
    };
    // Columns left out of `among`: held without a test, but the gauge leaves out those in
    // progress as the pick testing every column does (the same count under both paths).
    let mut untested = 0;
    match among {
        Some(Among { columns, dirty_in_progress }) => {
            let mut tested = 0;
            for (&key, &marks) in columns.iter().filter_map(|k| dirty.get(k).map(|m| (k, m))) {
                tested += 1;
                classify(key, marks);
            }
            let untested_in_progress = dirty_in_progress.saturating_sub(in_progress);
            untested = (dirty.len() - tested).saturating_sub(untested_in_progress);
        }
        None => dirty.iter().for_each(|(&key, &marks)| classify(key, marks)),
    }
    let held_count = held + untested;
    keep_closest(&mut ready, free, eye);
    let mut left_marked = 0;
    for &(key, launch) in &ready {
        let Some(marks) = dirty.get_mut(&key) else { continue };
        marks.subtract(launch);
        if marks.is_empty() {
            dirty.remove(&key);
        } else {
            left_marked += 1;
        }
    }
    Picked { launches: ready, held: held_count, partly_launched: left_marked }
}

/// Keeps the `n` launches closest to the eye column, sorted from the closest.
fn keep_closest(launches: &mut Vec<Launch>, n: usize, eye: (i32, i32)) {
    let dist = |&(k, _): &Launch| remesh::column_dist2(k, eye);
    if launches.len() > n {
        launches.select_nth_unstable_by_key(n.saturating_sub(1), dist);
        launches.truncate(n);
    }
    launches.sort_unstable_by_key(dist);
}

/// One meshing task: run on the pool, or on the main thread for a priority remesh.
struct MeshTask {
    job: MeshJob,
    tables: Arc<Tables>,
    palette: Arc<BiomePalette>,
    min_y: i32,
    /// Redone for changed blocks or light (kept as a base), not the whole column.
    live: bool,
    urgent: bool,
    /// Version of the base the partial remesh starts from.
    base_version: Option<u64>,
    kind: perf_log::MeshJobKind,
    /// Sections meshed (perf log).
    sections: usize,
}

impl MeshTask {
    /// Meshes, then prepares the upload (`remesh::finish`). With `release_job_early`, the job's
    /// nine columns are let go right after the meshing: they are shared with the client world,
    /// whose next write to a column a task still holds copies it whole, and the end of the task
    /// (flattening, GPU bytes) reads none of them. Otherwise the job is handed back, to be dropped
    /// after the send as before.
    fn run(self, release_job_early: bool) -> (remesh::Done, Option<MeshJob>) {
        let MeshTask { job, tables, palette, min_y, live, urgent, base_version, kind, sections } = self;
        let t = Instant::now();
        let mesh = blocks::mesh(&job, &tables, &palette, min_y);
        perf_log::mesh_work(t.elapsed(), sections, kind);
        let (reuse, kept_job) = take_base(job, release_job_early);
        let t = Instant::now();
        let base = reuse.as_ref().zip(base_version).map(|((remeshed, packed), version)| remesh::PartialBase {
            packed,
            version,
            remeshed: *remeshed,
        });
        let done = remesh::finish(mesh, live, urgent, base);
        perf_log::prep_work(t.elapsed());
        (done, kept_job)
    }
}

/// The base of a meshed job, and the job itself unless `release` lets it (and its columns) go now.
fn take_base(mut job: MeshJob, release: bool) -> (Option<(Sections, blocks::Packed)>, Option<MeshJob>) {
    let reuse = job.reuse.take();
    if release {
        drop(job);
        return (reuse, None);
    }
    (reuse, Some(job))
}

#[cfg(test)]
#[path = "app_meshing_filter_test.rs"]
mod filter_tests;

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
