//! Per-section remeshing.
//!
//! Meshing a block reads the light of its one-block neighbourhood (smooth lighting) and the states
//! up to two blocks away (face removal, one-block fluids; ambient occlusion tests the opacity
//! of the corners behind the neighbouring cell, two cells away along the normal); the sky light
//! of a section without data is read from the bottom row of the first section above it that
//! has some. A change therefore remeshes only the sections that read a changed cell: its own, and
//! its neighbours when the cell is on an edge (light) or two cells from an edge (state). The
//! other sections of the column are taken from the previous mesh (kept in a bounded cache;
//! without it, whole column).
//!
//! A single partial mesh in progress per column: the next one starts from its result, so the
//! sections reused are always those of the last mesh received (a whole column to redo
//! starts without waiting). Like the game, a column never meshed waits for its 8 neighbours.

use hashbrown::{HashMap, HashSet};
use std::collections::BTreeMap;
use std::sync::Arc;

use rc_mesh::blocks::BlockMesh;
use rc_mesh::wire::Sections;
use rc_world::{LIGHT_BYTES, LightLayer, edge};

use crate::World;

/// Budget of the mesh cache (MB), `RUSTCRAFT_MESHCACHE_MB` to change it (0: no cache,
/// whole columns as before). 64 MB (bench of 19 October, `docs/perf.md`): plains32 private memory
/// 2,360 / 2,499 / 2,588 MB at 32 / 64 / 128 MB, caves pool 497 / 370 / 278 ms/s.
fn budget() -> usize {
    rc_data::env_str!("RUSTCRAFT_MESHCACHE_MB").and_then(|v| v.parse().ok()).unwrap_or(64) << 20
}

/// Flattening of the base of partial remeshes (`RUSTCRAFT_OPT_MESHPACK`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackAt {
    /// `0`: old path, buffer reused then copied into the base (main thread).
    Copy,
    /// `main`: new buffer of the exact size, kept as is (main thread, no copy).
    Main,
    /// Default: flattening on the pool, in the meshing task; the mesh is also given back
    /// to the pool to be freed there after the upload.
    Pool,
}

/// Switches of the mesh upload (`docs/perf.md`), read once at startup; `=0`
/// goes back to the old path.
#[derive(Debug)]
pub struct Opts {
    pub pack: PackAt,
    /// `RUSTCRAFT_OPT_MESHPREP`: GPU bytes prepared on the pool.
    pub prep: bool,
    /// `RUSTCRAFT_OPT_TRANSFREE`: translucent centres neither computed nor kept for a column
    /// whose order does not change the image (direct identity order).
    pub transfree: bool,
    /// `RUSTCRAFT_OPT_PARTIALINPLACE`: partial remesh of the same size written in place.
    pub inplace: bool,
    /// `RUSTCRAFT_OPT_UPLOADBUDGET`: bytes uploaded per frame for whole columns
    /// (`RUSTCRAFT_UPLOADBUDGET_MB`, 16 by default); `None`: everything on every frame.
    pub budget: Option<usize>,
}

pub fn opts() -> &'static Opts {
    static O: std::sync::OnceLock<Opts> = std::sync::OnceLock::new();
    O.get_or_init(|| {
        let var = |n: &str| rc_data::env_flags::read_os(n).and_then(|v| v.into_string().ok());
        let on = |n: &str| var(n).as_deref() != Some("0");
        let pack = match var("RUSTCRAFT_OPT_MESHPACK").as_deref() {
            Some("0") => PackAt::Copy,
            Some("main") => PackAt::Main,
            _ => PackAt::Pool,
        };
        let mb = var("RUSTCRAFT_UPLOADBUDGET_MB").and_then(|v| v.parse::<usize>().ok()).unwrap_or(16);
        let o = Opts {
            pack,
            prep: on("RUSTCRAFT_OPT_MESHPREP"),
            transfree: on("RUSTCRAFT_OPT_TRANSFREE"),
            inplace: on("RUSTCRAFT_OPT_PARTIALINPLACE"),
            budget: on("RUSTCRAFT_OPT_UPLOADBUDGET").then_some(mb << 20),
        };
        eprintln!("[opt] mesh upload: {o:?}");
        o
    })
}

/// Mesh formats (same image): offset models in compact faces
/// (`RUSTCRAFT_OPT_OFFSETFACES`), back face of water by one bit (`RUSTCRAFT_OPT_WATERDOUBLE`).
/// Read at startup; `flip_formats` (`meshflip` step of `RUSTCRAFT_CMDTEST`) flips them to
/// compare two images in a single launch (the shader reads both formats).
static FORMATS: std::sync::OnceLock<[std::sync::atomic::AtomicBool; 2]> = std::sync::OnceLock::new();

fn formats_cell() -> &'static [std::sync::atomic::AtomicBool; 2] {
    FORMATS.get_or_init(|| {
        let on = |n: &str| rc_data::env_flags::read_os(n).is_none_or(|v| v != "0");
        let f = [on("RUSTCRAFT_OPT_OFFSETFACES"), on("RUSTCRAFT_OPT_WATERDOUBLE")];
        eprintln!("[opt] mesh formats: offset faces {}, double-sided water {}", f[0], f[1]);
        f.map(std::sync::atomic::AtomicBool::new)
    })
}

/// (compact offset faces, double-sided water) in force.
pub fn formats() -> (bool, bool) {
    let f = formats_cell();
    (f[0].load(std::sync::atomic::Ordering::Relaxed), f[1].load(std::sync::atomic::Ordering::Relaxed))
}

/// Flips the two formats (comparison in a single launch); offset faces kept if the
/// compact fluids depend on them (`fluid96`).
pub fn flip_formats(keep_offset: bool) {
    let f = formats_cell();
    if !keep_offset {
        f[0].fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
    }
    f[1].fetch_xor(true, std::sync::atomic::Ordering::Relaxed);
    eprintln!("[opt] mesh formats flipped: {:?}", formats());
}

/// Mesh made by the pool, and what it prepared for the main thread.
pub struct Done {
    pub mesh: BlockMesh,
    /// Redone for changed blocks or light (not the whole column): kept as a base.
    pub live: bool,
    /// Priority update (`prioritizeChunkUpdates`), meshed on the main thread to be
    /// seen on this frame: never delayed by the budget, like a partial remesh.
    pub urgent: bool,
    /// Flat base, made on the pool (`PackAt::Pool`, `live` meshes).
    pub packed: Option<rc_mesh::blocks::Packed>,
    /// GPU bytes prepared on the pool (`RUSTCRAFT_OPT_MESHPREP`).
    pub prep: Option<crate::render::Prepared>,
    /// Partial remesh of the same size as its base: version of the base and ranges to rewrite.
    pub in_place: Option<(u64, rc_mesh::blocks::InPlace)>,
}

impl Done {
    /// Uploaded on the frame of its arrival, whatever the budget.
    fn never_delayed(&self) -> bool {
        self.live || self.urgent
    }

    /// Bytes to upload (budget per frame).
    pub fn bytes(&self) -> usize {
        match &self.prep {
            Some(p) => p.bytes(),
            None => {
                let f = crate::render::gpu_formats();
                let m = &self.mesh;
                m.layers.iter().map(Vec::len).sum::<usize>() * 24
                    + (m.fluids[0].len() + m.fluids[1].len()) * f.fluid_words() * 4
                    + m.fluids[2].len() * f.trans_words() * 4
            }
        }
    }
}

/// Squared distance between columns, in i64: a distant teleport (50,000 columns) would overflow
/// i32.
pub fn column_dist2((x, z): (i32, i32), (ex, ez): (i32, i32)) -> i64 {
    (i64::from(x) - i64::from(ex)).pow(2) + (i64::from(z) - i64::from(ez)).pow(2)
}

/// Upload budget of a frame: partial and priority remeshes never delayed, then whole columns from the
/// closest to column `eye` up to `budget` bytes (at least one). `batch` keeps what
/// leaves on this frame; the rest is given back (it waits for the next frame, still "in progress").
pub fn split_budget(batch: &mut Vec<Done>, eye: (i32, i32), budget: usize) -> Vec<Done> {
    if batch.len() < 2 {
        return Vec::new();
    }
    batch.sort_by_key(|d| (!d.never_delayed(), column_dist2((d.mesh.x, d.mesh.z), eye)));
    let (mut used, mut cut) = (0, batch.len());
    for (i, d) in batch.iter().enumerate() {
        if !d.never_delayed() && used >= budget && i > 0 {
            cut = i;
            break;
        }
        used += d.bytes();
    }
    batch.split_off(cut)
}

/// End of the pool task: flattening of the base, GPU bytes and in-place write prepared
/// according to the switches. `base`: base of the partial remesh and its version.
pub fn finish(mut mesh: BlockMesh, live: bool, urgent: bool, base: Option<(&rc_mesh::blocks::Packed, u64)>) -> Done {
    let o = opts();
    let packed = (live && o.pack == PackAt::Pool)
        .then(|| Arc::new(rc_mesh::blocks::pack_exact(&mesh)) as rc_mesh::blocks::Packed);
    let in_place =
        base.filter(|_| o.inplace).and_then(|(w, v)| rc_mesh::blocks::in_place_diff(w, &mesh).map(|d| (v, d)));
    let prep = o.prep.then(|| crate::render::prepare(&mesh, o.transfree, crate::render::gpu_formats()));
    // Faces and fluids that became useless on the main thread: freed here (the check keeps everything
    // for its fingerprints).
    if prep.is_some() && (!live || packed.is_some()) && !checking() {
        mesh.layers = Default::default();
        mesh.fluids = Default::default();
    }
    Done { mesh, live, urgent, packed, prep, in_place }
}

/// Mesh kept flat, its usage rank and its version.
type Kept = (rc_mesh::blocks::Packed, u64, u64);

/// Meshes in progress and last meshes received.
pub struct Meshes {
    /// Column → version of the mesh in progress, and whether it redoes changed sections (not the whole
    /// column).
    busy: HashMap<(i32, i32), (u64, bool)>,
    /// Last mesh received for the columns that change (base of the partial remeshes), stored
    /// flat (`rc_mesh::blocks::pack`), with its usage rank; `lru` orders them from the oldest
    /// to the most recent.
    cache: HashMap<(i32, i32), Kept>,
    lru: BTreeMap<u64, (i32, i32)>,
    bytes: usize,
    budget: usize,
    clock: u64,
    /// Flattening buffer reused.
    scratch: Vec<u32>,
    /// Columns already meshed once since their loading (the game compiles a section
    /// never compiled before only if the 8 columns around it are loaded).
    compiled: HashSet<(i32, i32)>,
    /// Columns never meshed with a neighbour missing: sections to do, waiting for the
    /// last neighbour (outside `World::dirty`, so as not to pass them again on every frame).
    waiting: HashMap<(i32, i32), Sections>,
    /// `RUSTCRAFT_MESHCHECK` check only: last mesh sent for each column, flat
    /// (detail of the differing sections).
    checked: HashMap<(i32, i32), Vec<u32>>,
    /// Check only: columns read by each mesh launched (centre then neighbours), and
    /// those of the last mesh sent for each column.
    jobs_seen: HashMap<((i32, i32), u64), Snapshot>,
    checked_world: HashMap<(i32, i32), Snapshot>,
}

type Snapshot = (Arc<rc_world::Column>, [Option<Arc<rc_world::Column>>; 8]);

impl Default for Meshes {
    fn default() -> Self {
        Meshes {
            busy: HashMap::new(),
            cache: HashMap::new(),
            lru: BTreeMap::new(),
            bytes: 0,
            budget: budget(),
            clock: 0,
            scratch: Vec::new(),
            compiled: HashSet::new(),
            waiting: HashMap::new(),
            checked: HashMap::new(),
            jobs_seen: HashMap::new(),
            checked_world: HashMap::new(),
        }
    }
}

impl Meshes {
    /// Column that can be launched: no mesh in progress, or a whole column to redo (which
    /// starts from no base: the mesh in progress will be ignored on arrival).
    pub fn may_start(&self, key: (i32, i32), mask: Sections) -> bool {
        mask.is_all() || !self.busy.contains_key(&key)
    }

    fn drop_entry(&mut self, key: (i32, i32)) {
        if let Some((w, stamp, _)) = self.cache.remove(&key) {
            self.lru.remove(&stamp);
            self.bytes -= w.len() * 4;
        }
    }

    /// Mesh received. `live`: redone for changed blocks or light; only kept one (the bulk
    /// of the columns, meshed whole at loading, never changes). A whole mesh that is not
    /// kept removes the old one (it is no longer the base). `packed`: already flattened by the pool.
    fn put(&mut self, key: (i32, i32), m: &BlockMesh, live: bool, packed: Option<rc_mesh::blocks::Packed>) {
        self.drop_entry(key);
        if live {
            let w: rc_mesh::blocks::Packed = match (packed, opts().pack) {
                (Some(w), _) => w,
                (None, PackAt::Copy) => {
                    rc_mesh::blocks::pack(m, &mut self.scratch);
                    Arc::new(self.scratch.clone())
                }
                (None, _) => Arc::new(rc_mesh::blocks::pack_exact(m)),
            };
            let size = w.len() * 4;
            if size <= self.budget {
                self.clock += 1;
                self.cache.insert(key, (w, self.clock, m.version));
                self.lru.insert(self.clock, key);
                self.bytes += size;
                while self.bytes > self.budget {
                    let Some((_, old)) = self.lru.pop_first() else { break };
                    if let Some((w, _, _)) = self.cache.remove(&old) {
                        self.bytes -= w.len() * 4;
                    }
                }
            }
        }
        crate::perf_log::mesh_cache(self.bytes, self.cache.len());
    }

    /// Kept base of the column, flat (usage refreshed), and its version.
    fn base(&mut self, key: (i32, i32)) -> Option<(rc_mesh::blocks::Packed, u64)> {
        let (w, stamp, version) = self.cache.get_mut(&key)?;
        self.lru.remove(stamp);
        self.clock += 1;
        *stamp = self.clock;
        self.lru.insert(self.clock, key);
        Some((w.clone(), *version))
    }

    /// Column forgotten: its mesh is no longer a valid base, and when it comes back it will wait
    /// for its neighbours again.
    pub fn forget(&mut self, key: (i32, i32)) {
        self.drop_entry(key);
        // Mesh still in progress: ignored on arrival (column from before the unloading).
        self.busy.remove(&key);
        self.compiled.remove(&key);
        self.waiting.remove(&key);
        self.checked.remove(&key);
    }

    /// New world: no base any more; the meshes still in progress will be ignored.
    pub fn clear(&mut self) {
        self.busy.clear();
        self.cache.clear();
        self.lru.clear();
        self.bytes = 0;
        self.compiled.clear();
        self.waiting.clear();
        self.checked.clear();
    }

    /// Check: columns read by a launched mesh.
    pub fn note_job(&mut self, job: &rc_mesh::wire::MeshJob) {
        if checking() {
            let key = ((job.center.x, job.center.z), job.version);
            self.jobs_seen.insert(key, (job.center.clone(), job.neighbors.clone()));
        }
    }

    /// Columns waiting for a neighbour (checks).
    pub fn waiting(&self, key: (i32, i32)) -> bool {
        self.waiting.contains_key(&key)
    }

    /// Number of columns waiting for a neighbour.
    pub fn waiting_len(&self) -> usize {
        self.waiting.len()
    }
}

impl World {
    fn sections(&self) -> i32 {
        self.height / 16
    }

    /// The 8 columns around (`x`, `z`) are loaded.
    fn surrounded(&self, x: i32, z: i32) -> bool {
        rc_mesh::wire::NEIGHBORS.iter().all(|&(dx, dz)| self.grid.get(x + dx, z + dz).is_some())
    }

    /// Sections `mask` of the column to remesh (new version, like `touch`). A column
    /// never meshed yet waits for its 8 neighbours to be loaded (game rule): its
    /// sections are set aside and resumed when the last neighbour arrives (which then touches
    /// the whole column, `touch_with_neighbors`).
    pub(crate) fn touch_sections(&mut self, x: i32, z: i32, mask: Sections) {
        if mask.is_empty() {
            return;
        }
        let key = (x, z);
        self.counter += 1;
        self.versions.insert(key, self.counter);
        if traced(x, z, false) {
            eprintln!("[meshtrace] marks v{} sections {:?}", self.counter, mask);
        }
        let m = &mut self.meshes;
        let started = m.compiled.contains(&key) || m.busy.contains_key(&key);
        if !started && !self.surrounded(x, z) {
            self.meshes.waiting.entry(key).or_default().union(mask);
            return;
        }
        let mut mask = mask;
        if let Some(w) = self.meshes.waiting.remove(&key) {
            mask.union(w);
        }
        self.dirty.entry(key).or_default().union(mask);
    }

    /// One or more cells of section `sy` (from the bottom of the world, −1 and `sections` for
    /// the light layers outside the world) of column (`cx`, `cz`) changed, on the edges
    /// `edges`: sections that read them marked, in this column and its neighbours. `light`:
    /// light cells (also read by the sections without sky data below).
    pub(crate) fn touch_around(&mut self, cx: i32, sy: i32, cz: i32, edges: u8, light: bool) {
        if traced(cx, cz, true) {
            eprintln!("[meshtrace] change ({cx}, {cz}) section {sy} edges {edges:#09b} light {light}");
        }
        let col = self.grid.get(cx, cz);
        let sky_data = |i: i32| {
            col.is_some_and(|c| usize::try_from(i).ok().and_then(|i| c.sky_light.get(i)).is_some_and(Option::is_some))
        };
        let Some((mask, xs, zs)) = rc_mesh::wire::reach(edges, sy, self.sections(), light, sky_data) else { return };
        for dx in xs {
            for dz in zs.clone() {
                if self.grid.get(cx + dx, cz + dz).is_some() {
                    self.touch_sections(cx + dx, cz + dz, mask);
                }
            }
        }
    }

    /// Blocks changed in section `sy` (indices of the section): sections that read them (states
    /// read up to two cells away by the ambient occlusion, hence `near_of_index`).
    pub(crate) fn touch_blocks(&mut self, cx: i32, sy: i32, cz: i32, changed: impl Iterator<Item = u16>) {
        let mut seen = 0u128;
        for i in changed {
            let e = edge::near_of_index(usize::from(i));
            if seen >> e & 1 == 0 {
                seen |= 1 << e;
                self.touch_around(cx, sy, cz, e, false);
            }
        }
    }

    /// Light received from the server for the column: sections where a cell changes (compared with the
    /// current light), before the replacement. Layer appeared: the whole section. True if a received
    /// layer differs from the column's (or column absent). `same_arc`: a layer
    /// shared with the column's is identical without comparing its bytes (`LIGHTNOCOPY`).
    pub(crate) fn touch_light(
        &mut self,
        cx: i32,
        cz: i32,
        sky: &[LightLayer],
        block: &[LightLayer],
        same_arc: bool,
    ) -> bool {
        let Some(col) = self.grid.get(cx, cz) else { return true };
        let mut marks: Vec<(i32, u8)> = Vec::new();
        let mut changed = false;
        for (old, new) in [(&col.sky_light, sky), (&col.block_light, block)] {
            for (i, n) in new.iter().enumerate() {
                let Some(n) = n else { continue };
                let edges = match old.get(i) {
                    Some(Some(o)) if same_arc && std::sync::Arc::ptr_eq(o, n) => 0,
                    Some(Some(o)) => light_edges(o, n),
                    _ => edge::ALL,
                };
                // Position outside the column: nothing will be placed there (remesh kept as is).
                changed |= edges != 0 && i < old.len();
                if edges != 0 {
                    marks.push((i as i32 - 1, edges));
                }
            }
        }
        for (sy, edges) in marks {
            self.touch_around(cx, sy, cz, edges, true);
        }
        changed
    }

    /// Meshing task of the column: sections to redo, and the previous mesh (with its
    /// version) if it allows redoing only them. Marks the column as in progress.
    pub(crate) fn mesh_reuse(
        &mut self,
        key: (i32, i32),
        mask: Sections,
        version: u64,
    ) -> Option<(Sections, (rc_mesh::blocks::Packed, u64))> {
        self.meshes.busy.insert(key, (version, !mask.is_all()));
        if mask.is_all() {
            return None;
        }
        let base = self.meshes.base(key);
        crate::perf_log::mesh_base(base.is_some());
        if traced(key.0, key.1, false) {
            eprintln!("[meshtrace] launches v{version} sections {mask:?} base {}", base.is_some());
        }
        Some((mask, base?))
    }

    /// Mesh received: `Some` if it must be uploaded to the GPU (and it becomes the column's base).
    pub(crate) fn mesh_done(&mut self, mut d: Done) -> Option<Done> {
        let m = &d.mesh;
        let key = (m.x, m.z);
        if traced(key.0, key.1, false) {
            eprintln!(
                "[meshtrace] received v{} (in progress {:?}, current {:?})",
                m.version,
                self.meshes.busy.get(&key),
                self.versions.get(&key)
            );
        }
        // Mesh of a previous world, or already replaced: ignored.
        let &(version, live) = self.meshes.busy.get(&key)?;
        if version != m.version {
            return None;
        }
        self.meshes.busy.remove(&key);
        let current = *self.versions.get(&key)?;
        self.meshes.compiled.insert(key);
        // Whole column to redo anyway (column replaced, settings): useless.
        if current != m.version && self.dirty.get(&key).is_some_and(Sections::is_all) {
            return None;
        }
        let packed = d.packed.take();
        if checking() {
            let mut w = Vec::new();
            rc_mesh::blocks::pack(m, &mut w);
            // Base flattened by the pool: identical to the main thread's.
            if let Some(p) = &packed
                && ***p != *w
            {
                eprintln!("[meshcheck] ({}, {}): base flattened by the pool differs", key.0, key.1);
            }
            if let Some(p) = &d.prep {
                p.check(m, key);
            }
            self.meshes.checked.insert(key, w);
            if let Some(s) = self.meshes.jobs_seen.remove(&(key, m.version)) {
                self.meshes.checked_world.insert(key, s);
            }
            self.meshes.jobs_seen.retain(|&(k, v), _| k != key || v > m.version);
        }
        self.meshes.put(key, m, live, packed);
        Some(d)
    }
}

/// Trace of a column (`RUSTCRAFT_MESHTRACE=x,z`): changes that reach it or reach
/// its neighbours, meshes launched and received (diagnostic of the check).
fn traced(x: i32, z: i32, around: bool) -> bool {
    static AT: std::sync::OnceLock<Option<(i32, i32)>> = std::sync::OnceLock::new();
    let at = AT.get_or_init(|| {
        let v = rc_data::env_str!("RUSTCRAFT_MESHTRACE")?;
        let (a, b) = v.split_once(',')?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    });
    at.is_some_and(|(tx, tz)| if around { (tx - x).abs() <= 1 && (tz - z).abs() <= 1 } else { (tx, tz) == (x, z) })
}

/// Check `RUSTCRAFT_MESHCHECK=1`: fingerprints of the meshes uploaded to the GPU, compared at the
/// final capture with a complete mesh redone from the world at that moment.
pub fn checking() -> bool {
    rc_data::env_flag!("RUSTCRAFT_MESHCHECK")
}

pub use rc_mesh::blocks::digest;

/// Meshes received since the pool's last memory reclaim, and its instant.
static RECLAIM: std::sync::Mutex<(u32, Option<std::time::Instant>)> = std::sync::Mutex::new((0, None));

/// A mesh received (and soon freed by the main thread).
pub fn note_received() {
    if let Ok(mut r) = RECLAIM.lock() {
        r.0 += 1;
    }
}

/// Meshes are allocated by the pool threads and freed by the main thread: mimalloc
/// keeps these blocks in the deferred list of the thread that allocated them until its next
/// allocation. With per-section remeshing, the pool stays idle after loading and
/// this memory was never reclaimed (+300 to 400 MB of private memory). At most once per
/// second, after at least 32 meshes received, each pool thread reclaims it (background task).
pub fn reclaim_pool() {
    let Ok(mut r) = RECLAIM.lock() else { return };
    if r.0 < 32 || r.1.is_some_and(|t| t.elapsed().as_secs_f64() < 1.0) {
        return;
    }
    *r = (0, Some(std::time::Instant::now()));
    rayon::spawn_broadcast(|_| {
        let t = std::time::Instant::now();
        crate::debug_sys::reclaim_freed();
        crate::perf_log::reclaim_work(t.elapsed());
    });
}

impl crate::App {
    /// Report of the check at the final capture: GPU columns identical to the complete mesh
    /// of the current world, and walk identical to a redone walk.
    pub(crate) fn mesh_check_report(&self) {
        use rayon::prelude::*;
        let Some(r) = &self.renderer else { return };
        let (gpu, reached) = r.mesh_check();
        let (grid, tables, palette, min_y) = (&self.world.grid, &*self.tables, &*self.palette, self.world.min_y);
        let (shade, has_sky, opts, plains) = (self.shade, self.env.has_sky, self.video.mesh, self.env.plains());
        let seed = crate::environ::biome_zoom_seed();
        let jobs: Vec<rc_mesh::wire::MeshJob> = grid
            .iter()
            .map(|c| {
                let center = grid.get_arc(c.x, c.z).expect("column present");
                let mut job = rc_mesh::wire::MeshJob::gather(center, 0, |x, z| grid.get_arc(x, z));
                (job.shade, job.has_sky, job.opts, job.plains_biome) = (shade, has_sky, opts, plains);
                (job.biome_zoom_seed, job.min_y) = (seed, min_y);
                job
            })
            .collect();
        let fresh_meshes: Vec<BlockMesh> =
            jobs.par_iter().map(|job| rc_mesh::blocks::mesh(job, tables, palette, min_y)).collect();
        let fresh: Vec<((i32, i32), Option<u64>)> =
            fresh_meshes.iter().map(|m| ((m.x, m.z), (!m.is_empty()).then(|| digest(m)))).collect();
        let (mut same, mut differ, mut parked) = (0, Vec::new(), 0);
        for (k, d) in &fresh {
            if !gpu.contains_key(k) && self.world.meshes.waiting(*k) && !self.world.surrounded(k.0, k.1) {
                // Never meshed: one of its neighbours is missing (game rule).
                parked += 1;
            } else if gpu.get(k).copied() == *d || (!gpu.contains_key(k) && d.is_none()) {
                same += 1;
            } else {
                differ.push(*k);
            }
        }
        differ.sort_unstable();
        // Different columns surrounded by their 8 neighbours (the others border a forgotten
        // column: the game does not remesh the neighbours of an unloaded column).
        let inner = differ
            .iter()
            .filter(|&&(x, z)| rc_mesh::wire::NEIGHBORS.iter().all(|(dx, dz)| grid.get(x + dx, z + dz).is_some()))
            .count();
        eprintln!(
            "[meshcheck] {} columns: {same} identical to the complete mesh, {} different (of which {inner} without \
             an absent neighbour) {:?}, {parked} never meshed (absent neighbour); waiting {}; walk \
             identical: {reached:?}",
            fresh.len(),
            differ.len(),
            &differ[..differ.len().min(12)],
            self.world.dirty.len() + self.in_flight,
        );
        // Detail of the first different columns: sections whose faces differ.
        for k in differ.iter().take(4) {
            let Some(new) = fresh_meshes.iter().find(|m| (m.x, m.z) == *k) else { continue };
            let Some(old) = self.world.meshes.checked.get(k).and_then(|w| rc_mesh::blocks::unpack(w, 0)) else {
                eprintln!("[meshcheck] ({}, {}): no sent mesh kept", k.0, k.1);
                continue;
            };
            let n = new.vis.len();
            let faces = |m: &BlockMesh, s: usize| {
                let mut v = Vec::new();
                for l in 0..3 {
                    for g in 0..rc_mesh::blocks::BUCKETS {
                        let st = &m.starts[l];
                        if let (Some(&a), Some(&e)) = (st.get(g * (n + 1) + s), st.get(g * (n + 1) + s + 1)) {
                            v.extend(m.layers[l][a as usize..e as usize].iter().map(|f| (l, *f)));
                        }
                    }
                }
                v
            };
            let fluids = |m: &BlockMesh, s: usize| {
                (0..3).map(|l| m.fluid_starts[l].get(s + 1).copied().unwrap_or(0) - m.fluid_starts[l][s]).sum::<u32>()
            };
            // Cells of the neighbourhood read by the mesh sent and changed since.
            if let Some((c0, n0)) = self.world.meshes.checked_world.get(k) {
                let olds = std::iter::once((0, 0, Some(c0)))
                    .chain(rc_mesh::wire::NEIGHBORS.iter().zip(n0).map(|(&(dx, dz), c)| (dx, dz, c.as_ref())));
                let nib = |l: &rc_world::LightLayer, i: usize| l.as_ref().map(|l| (l[i >> 1] >> ((i & 1) * 4)) & 15);
                let mut shown = 0;
                for (dx, dz, old) in olds {
                    let (Some(old), Some(cur)) = (old, grid.get(k.0 + dx, k.1 + dz)) else { continue };
                    for y in 0..(cur.sections.len() * 16) {
                        for i in 0..256 {
                            let (x, z) = (i & 15, i >> 4);
                            let li = rc_world::block_index(x, y & 15, z);
                            let ly = (y >> 4) + 1;
                            let a = (old.block(x, y, z), nib(&old.sky_light[ly], li), nib(&old.block_light[ly], li));
                            let b = (cur.block(x, y, z), nib(&cur.sky_light[ly], li), nib(&cur.block_light[ly], li));
                            if a != b && shown < 24 {
                                shown += 1;
                                eprintln!(
                                    "[meshcheck]   column ({}, {}) cell ({x}, {y}, {z}): (state, sky, block) {a:?} -> {b:?}",
                                    k.0 + dx,
                                    k.1 + dz
                                );
                            }
                        }
                    }
                }
            }
            for s in 0..n {
                let (a, b) = (faces(&old, s), faces(new, s));
                let fl = (fluids(&old, s), fluids(new, s));
                let vis = old.vis.get(s) == new.vis.get(s);
                if a == b && fl.0 == fl.1 && vis {
                    continue;
                }
                let first = a.iter().zip(&b).find(|(x, y)| x != y).map(|(x, y)| (x.0, x.1, y.1));
                eprintln!(
                    "[meshcheck] ({}, {}) section {s}: GPU faces {} / full {}, fluids {:?}, identical communication {vis}, \
                     first different face (layer, GPU, full) {first:?}",
                    k.0,
                    k.1,
                    a.len(),
                    b.len(),
                    fl
                );
            }
        }
    }
}

/// Edges of the cells whose level differs between two layers (0 if identical). By 64-bit
/// words: one word = one row of 16 cells (x from 0 to 15, low nibble first) at fixed y and z;
/// word i: y = i / 16, z = i % 16.
fn light_edges(old: &[u8; LIGHT_BYTES], new: &[u8; LIGHT_BYTES]) -> u8 {
    if old == new {
        return 0;
    }
    let (o, n) = (old.as_chunks::<8>().0, new.as_chunks::<8>().0);
    let mut edges = 0u8;
    for (i, (o, n)) in o.iter().zip(n).enumerate() {
        let d = u64::from_le_bytes(*o) ^ u64::from_le_bytes(*n);
        if d == 0 {
            continue;
        }
        edges |= edge::ANY | edge::bits(i >> 4, edge::Y0) | edge::bits(i & 15, edge::Z0);
        edges |= if d & 0xf != 0 { edge::X0 } else { 0 } | if d >> 60 != 0 { edge::X15 } else { 0 };
    }
    edges
}

/// Cell-by-cell version of [`light_edges`] (test reference).
#[cfg(test)]
fn light_edges_cells(old: &[u8; LIGHT_BYTES], new: &[u8; LIGHT_BYTES]) -> u8 {
    if old == new {
        return 0;
    }
    let mut edges = edge::ANY;
    for (b, (o, n)) in old.iter().zip(new).enumerate() {
        let d = o ^ n;
        if d == 0 {
            continue;
        }
        for half in 0..2 {
            if d >> (half * 4) & 15 != 0 {
                edges |= edge::of_index(b * 2 + half);
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `touch_light` (C2-M05, `LIGHTNOCOPY`): true only if a received layer differs from the
    /// column's; an identical shared layer is recognised; same sections to remesh with
    /// and without the shortcut.
    #[test]
    fn touch_light_reports_real_changes() {
        let layer = |v: u8| Some(Arc::new([v; LIGHT_BYTES]));
        let a = layer(0x11);
        let col = rc_world::Column {
            x: 0,
            z: 0,
            sections: vec![rc_world::Section::empty(0, 0); 2].into_boxed_slice(),
            sky_light: vec![None, a.clone(), layer(0), None].into_boxed_slice(),
            block_light: vec![None; 4].into_boxed_slice(),
            heightmaps: Vec::new(),
        };
        let none = [None, None, None, None];
        let mut dirty = Vec::new();
        for same_arc in [false, true] {
            let mut w = World {
                grid: rc_world::ChunkGrid::new(2),
                min_y: 0,
                height: 32,
                state_bits: 15,
                versions: HashMap::new(),
                dirty: HashMap::new(),
                meshes: Default::default(),
                counter: 0,
                hm_rules: Vec::new(),
            };
            w.grid.insert(col.clone());
            assert!(!w.touch_light(0, 0, &[None, a.clone(), None, None], &none, same_arc), "same shared layer");
            assert!(!w.touch_light(0, 0, &[None, layer(0x11), layer(0), None], &none, same_arc), "same bytes");
            // Column without its 8 neighbours: sections set aside (`waiting`), version moved.
            assert!(w.versions.is_empty(), "identical layers: nothing to remesh");
            assert!(!w.touch_light(0, 0, &[None, None, None, None, layer(1)], &none, same_arc), "outside the column");
            assert!(w.touch_light(0, 0, &[None, layer(0x12), None, None], &none, same_arc), "layer changed");
            assert!(w.meshes.waiting.contains_key(&(0, 0)));
            assert!(w.touch_light(0, 0, &none, &[None, layer(0), None, None], same_arc), "layer appeared");
            assert!(w.touch_light(7, 7, &none, &none, same_arc), "column absent");
            let mut d: Vec<_> = w.meshes.waiting.iter().map(|(k, m)| (*k, format!("{m:?}"))).collect();
            d.sort();
            dirty.push(d);
        }
        assert_eq!(dirty[0], dirty[1], "different sections to remesh");
    }

    #[test]
    fn pack_round_trip() {
        let q = rc_mesh::fluid::FluidQuad {
            corners: std::array::from_fn(|c| std::array::from_fn(|k| (c * 5 + k) as f32 * 0.37 - 2.0)),
            texture: 77,
            colors: [1, 0xffff_ffff, 3, 4],
            light: [0xf0f0, 1, 0x00ff, 9],
        };
        let m = BlockMesh {
            x: 3,
            z: -4,
            version: 9,
            layers: [vec![[1, 2, 3, 4, 5, 6]; 3], Vec::new(), vec![[7; 6]]],
            fluids: [vec![q; 2], Vec::new(), vec![q]],
            starts: [vec![0, 1, 3], vec![0, 0, 0], vec![0, 1, 1]],
            fluid_starts: [vec![0, 2], vec![0, 0], vec![0, 1]],
            vis: vec![u64::MAX - 5, 42],
        };
        let mut w = Vec::new();
        rc_mesh::blocks::pack(&m, &mut w);
        let b = rc_mesh::blocks::unpack(&w, 9).expect("flat");
        assert!(b.x == 3 && b.z == -4 && b.layers == m.layers && b.fluids == m.fluids && b.starts == m.starts);
        assert!(b.fluid_starts == m.fluid_starts && b.vis == m.vis);
    }

    /// Upload budget: nothing lost or doubled when the queue empties frame after frame, the partial
    /// remeshes always leave on the frame of their arrival, at least one column per
    /// frame, whole columns from the closest to the farthest.
    #[test]
    fn upload_budget_loses_nothing_and_never_delays_partials() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let done = |x: i32, z: i32, faces: usize, (live, urgent): (bool, bool), version: u64| Done {
            mesh: BlockMesh {
                x,
                z,
                version,
                layers: [vec![[0; 6]; faces], Vec::new(), Vec::new()],
                fluids: Default::default(),
                starts: Default::default(),
                fluid_starts: Default::default(),
                vis: Vec::new(),
            },
            live,
            urgent,
            packed: None,
            prep: None,
            in_place: None,
        };
        let mut version = 0;
        for round in 0..200 {
            let budget = 24 * (1 + next(4000) as usize);
            // One round in four, eye teleported 50,000 columns away (square beyond i32).
            let far = if round % 4 == 0 { 50_000 } else { 0 };
            let eye = (far + next(20) as i32 - 10, next(20) as i32 - 10);
            let mut pending: Vec<Done> = Vec::new();
            let mut seen = Vec::new();
            let mut sent = Vec::new();
            for frame in 0.. {
                if frame >= 20 && pending.is_empty() {
                    break;
                }
                assert!(frame < 5000, "queue never emptied");
                let mut batch = std::mem::take(&mut pending);
                if frame < 20 {
                    for _ in 0..next(30) {
                        version += 1;
                        let live = next(4) == 0;
                        let urgent = !live && next(6) == 0;
                        // Columns around the eye, or left near the origin.
                        let x0 = if far > 0 && next(2) == 0 { far } else { 0 };
                        batch.push(done(
                            x0 + next(40) as i32 - 20,
                            next(40) as i32 - 20,
                            next(2000) as usize,
                            (live, urgent),
                            version,
                        ));
                        seen.push(version);
                    }
                }
                let arrived: Vec<u64> = batch.iter().filter(|d| d.live || d.urgent).map(|d| d.mesh.version).collect();
                let had = !batch.is_empty();
                pending = split_budget(&mut batch, eye, budget);
                assert!(!had || !batch.is_empty(), "frame without any upload");
                assert!(pending.iter().all(|d| !d.live && !d.urgent), "partial or priority remesh delayed");
                assert!(arrived.iter().all(|v| batch.iter().any(|d| d.mesh.version == *v)));
                let dist = |d: &Done| {
                    let (dx, dz) = (f64::from(d.mesh.x) - f64::from(eye.0), f64::from(d.mesh.z) - f64::from(eye.1));
                    dx * dx + dz * dz
                };
                let sent_max = batch.iter().filter(|d| !d.never_delayed()).map(dist).max_by(f64::total_cmp);
                let left_min = pending.iter().map(dist).min_by(f64::total_cmp);
                if let (Some(a), Some(b)) = (sent_max, left_min) {
                    assert!(a <= b, "closer column left for a farther one");
                }
                let full: usize = batch.iter().filter(|d| !d.never_delayed()).map(Done::bytes).sum();
                let last = batch.iter().filter(|d| !d.never_delayed()).map(Done::bytes).next_back().unwrap_or(0);
                assert!(
                    full - last < budget || batch.iter().filter(|d| !d.never_delayed()).count() <= 1,
                    "budget exceeded"
                );
                sent.extend(batch.iter().map(|d| d.mesh.version));
            }
            sent.sort_unstable();
            seen.sort_unstable();
            assert_eq!(sent, seen, "meshes lost or doubled");
        }
    }

    #[test]
    fn light_edges_follow_changed_cells() {
        let a = [0u8; LIGHT_BYTES];
        assert_eq!(light_edges(&a, &a), 0);
        let mut b = a;
        // Cell (x 3, y 7, z 9): interior.
        let i = (7 << 8) | (9 << 4) | 3;
        b[i >> 1] |= 5 << ((i & 1) * 4);
        assert_eq!(light_edges(&a, &b), edge::ANY);
        // Cell (x 15, y 0, z 0): three edges.
        let j = 15;
        b[j >> 1] |= 2 << ((j & 1) * 4);
        assert_eq!(light_edges(&a, &b), edge::ANY | edge::X15 | edge::Y0 | edge::Z0);
    }

    /// Cost of the two versions (`cargo test --release -p rc-app light_edges_cost -- --ignored
    /// --nocapture`): different layers everywhere (worst case, new light packet).
    #[test]
    #[ignore]
    fn light_edges_cost() {
        let a = [0x5au8; LIGHT_BYTES];
        let b = [0xa5u8; LIGHT_BYTES];
        let n = 200_000;
        let t = std::time::Instant::now();
        let mut acc = 0u32;
        for _ in 0..n {
            acc += u32::from(light_edges(std::hint::black_box(&a), std::hint::black_box(&b)));
        }
        let words = t.elapsed().as_nanos() as f64 / f64::from(n);
        let t = std::time::Instant::now();
        for _ in 0..n {
            acc += u32::from(light_edges_cells(std::hint::black_box(&a), std::hint::black_box(&b)));
        }
        let cells = t.elapsed().as_nanos() as f64 / f64::from(n);
        eprintln!("light_edges: words {words:.0} ns, cells {cells:.0} ns per layer ({acc})");
    }

    /// Word version identical to the cell-by-cell version, on randomly drawn layers
    /// (a few cells changed, often on the edges).
    #[test]
    fn light_edges_words_match_cells() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut a = [0u8; LIGHT_BYTES];
        a.iter_mut().for_each(|v| *v = next() as u8);
        for _ in 0..4000 {
            let mut b = a;
            for _ in 0..(next() % 4) {
                let pick = |r: u64| match r % 3 {
                    0 => 0,
                    1 => 15,
                    _ => (r >> 8) as usize % 16,
                };
                let (x, y, z) = (pick(next()), pick(next()), pick(next()));
                let i = (y << 8) | (z << 4) | x;
                b[i >> 1] ^= ((next() % 15 + 1) as u8) << ((i & 1) * 4);
            }
            assert_eq!(light_edges(&a, &b), light_edges_cells(&a, &b));
        }
    }
}
