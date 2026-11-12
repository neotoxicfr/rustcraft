//! Folder of a world and its dimensions (`anvil-world.md` § 0): chunk loading in
//! batches, regions read in parallel on the pool.

use hashbrown::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rc_data::VersionData;

use crate::chunk::{self, Chunk, Height, Skip};
use crate::region::{ReadPlan, RegionFile, SharedRegion, SharedRegions, Unreadable, local_index, region_name};
use crate::states::States;

/// How a dimension reads its region folders (`RUSTCRAFT_OPT_REGION_*`, each read once; tests
/// build their own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoadPlan {
    pub read: ReadPlan,
    /// Each region file opened once per load and shared by its tasks, instead of reopened by
    /// every task (`RUSTCRAFT_OPT_REGION_SHARED_HANDLE`).
    pub shared_handle: bool,
    /// Those files kept open by the dimension from one load to the next
    /// (`RUSTCRAFT_OPT_REGION_HANDLE_CACHE`, only with `shared_handle`).
    pub handle_cache: bool,
}

impl LoadPlan {
    /// The old path: every task opens its region file, old read.
    #[cfg(test)]
    pub(crate) const OLD: LoadPlan = LoadPlan { read: ReadPlan::OLD, shared_handle: false, handle_cache: false };

    pub(crate) fn from_switches() -> LoadPlan {
        LoadPlan {
            read: ReadPlan::from_switches(),
            shared_handle: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_SHARED_HANDLE"),
            handle_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_HANDLE_CACHE"),
        }
    }
}

/// Region files opened for one load, by region coordinates (`None`: absent or unreadable file).
type OpenRegions = HashMap<(i32, i32), Option<std::sync::Arc<SharedRegion>>>;

/// Outcome of loading a chunk.
#[derive(Debug)]
pub enum Load {
    Absent,
    /// Present but not loadable (status, version, corruption): never rewritten.
    Skipped(Skip),
    Unreadable(Unreadable),
    Loaded(Box<Chunk>),
}

#[derive(Clone)]
pub struct Dimension {
    /// `minecraft:overworld`…
    pub name: String,
    pub dir: PathBuf,
    pub height: Height,
    /// Flags of the dimension type (data extracted from the game, immutable), read at first
    /// use instead of a name lookup at each call (`RUSTCRAFT_OPT_SIMLIGHTMISC`).
    pub(crate) kind: std::sync::OnceLock<DimKind>,
    /// Region files kept open for reading between loads (`RUSTCRAFT_OPT_REGION_HANDLE_CACHE`),
    /// shared by the clones; closed with the last clone or by [`Dimension::close_region_files`].
    pub(crate) region_files: std::sync::Arc<SharedRegions>,
    /// The overworld, by its name, decided once at the creation (`RUSTCRAFT_OPT_DIMFLAGS`).
    overworld: bool,
}

/// `RUSTCRAFT_OPT_DIMFLAGS` (default on): the overworld test of the hot mob paths reads a flag
/// decided when the dimension is created instead of comparing its name at every call. (The
/// Nether test it also covered is gone: the mobs' light reads the ambient light of the type.)
fn dimension_flags() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_DIMFLAGS")
}

/// Flags of a dimension type, same rules as the lookups they replace: unknown type
/// = sky, no ceiling, no "nether" lighting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DimKind {
    /// Sky light (first type of that name; absent: true).
    pub sky: bool,
    /// Ceiling (first type of that name; absent: false).
    pub ceiling: bool,
    /// Sky without "nether" lighting (first type of that name; absent: true): weather exists.
    pub weather: bool,
    /// Ambient light (first type of that name; absent: 0): how far the mobs' brightness curve
    /// is raised towards full light.
    pub ambient_light: f32,
}

impl DimKind {
    pub(crate) fn of(data: &VersionData, name: &str) -> DimKind {
        let t = data.dimension_types.iter().find(|t| t.name == name);
        DimKind {
            sky: t.is_none_or(|t| t.has_skylight),
            ceiling: t.is_some_and(|t| t.has_ceiling),
            weather: t.is_none_or(|t| t.has_skylight && !t.nether_light),
            ambient_light: t.map_or(0.0, |t| t.ambient_light),
        }
    }
}

/// Folder of a dimension: `dimensions/<namespace>/<name>`.
pub fn dimension_dir(world: &Path, name: &str) -> PathBuf {
    let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
    world.join("dimensions").join(ns).join(path)
}

impl Dimension {
    pub fn new(world: &Path, name: &str, height: Height) -> Dimension {
        Dimension {
            name: name.to_owned(),
            dir: dimension_dir(world, name),
            height,
            kind: std::sync::OnceLock::new(),
            region_files: Default::default(),
            overworld: name == crate::dims::OVERWORLD,
        }
    }

    /// The overworld (flag kept since the creation; old path: name compared).
    pub(crate) fn is_overworld(&self) -> bool {
        if !dimension_flags() {
            return self.name == crate::dims::OVERWORLD;
        }
        #[cfg(test)]
        assert_eq!(self.overworld, self.name == crate::dims::OVERWORLD, "dimension renamed without `rename`");
        self.overworld
    }

    /// Renames the dimension in a test (its flag follows).
    #[cfg(test)]
    pub(crate) fn rename(&mut self, name: &str) {
        self.name = name.to_owned();
        self.overworld = name == crate::dims::OVERWORLD;
    }

    /// Closes the region files kept for reading (world closed: its folder may be deleted, renamed
    /// or recreated next, which an open file prevents on Windows).
    pub fn close_region_files(&self) {
        self.region_files.close_all();
    }

    pub fn region_dir(&self) -> PathBuf {
        self.dir.join("region")
    }

    /// Region files present: region coordinates.
    pub fn regions(&self) -> Vec<(i32, i32)> {
        let Ok(rd) = std::fs::read_dir(self.region_dir()) else { return Vec::new() };
        rd.flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let mut p = name.strip_prefix("r.")?.strip_suffix(".mca")?.split('.');
                Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
            })
            .collect()
    }

    /// One chunk read on the calling thread: an occasional read from the server thread (the biome
    /// of a neighbour not loaded) skips the grouping by region and the two pool hand-overs of
    /// [`Dimension::load`], which cost more than the read of an absent chunk itself.
    pub(crate) fn load_one(&self, (x, z): (i32, i32), data: &VersionData, states: &States) -> Load {
        let plan = LoadPlan::from_switches();
        if !(plan.shared_handle && plan.handle_cache) {
            return self.load(&[(x, z)], data, states).pop().map_or(Load::Absent, |(_, load)| load);
        }
        let Some(file) = self.region_files.get(&self.region_dir().join(region_name(x, z))) else { return Load::Absent };
        read_one_shared(&file, x, z, plan.read, data, states, self.height)
    }

    /// Loads chunks: grouped by region, one pool task per region.
    pub fn load(&self, chunks: &[(i32, i32)], data: &VersionData, states: &States) -> Vec<((i32, i32), Load)> {
        self.load_split(chunks, data, states, usize::MAX)
    }

    /// Like [`Dimension::load`], each region split into tasks of at most `per_task` chunks: a
    /// small list taken from one or two regions then occupies several threads instead of one or
    /// two. Same outcomes, in the same order per region.
    pub fn load_split(
        &self,
        chunks: &[(i32, i32)],
        data: &VersionData,
        states: &States,
        per_task: usize,
    ) -> Vec<((i32, i32), Load)> {
        self.load_split_with(chunks, data, states, per_task, LoadPlan::from_switches())
    }

    pub(crate) fn load_split_with(
        &self,
        chunks: &[(i32, i32)],
        data: &VersionData,
        states: &States,
        per_task: usize,
        plan: LoadPlan,
    ) -> Vec<((i32, i32), Load)> {
        let dir = self.region_dir();
        let h = self.height;
        let tasks = tasks_by_region(chunks, per_task);
        if !plan.shared_handle {
            // Old path: every task opens its region file again.
            return tasks
                .into_par_iter()
                .flat_map_iter(|((rx, rz), list)| {
                    let path = dir.join(region_name(rx << 5, rz << 5));
                    let mut file = open_for_reading(&path, plan.read);
                    list.into_iter()
                        .map(|(x, z)| {
                            let Some(f) = file.as_mut() else { return ((x, z), Load::Absent) };
                            ((x, z), read_one(f, x, z, data, states, h))
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        let files = self.open_regions(&dir, &tasks, plan);
        tasks
            .into_par_iter()
            .flat_map_iter(|(r, list)| {
                let file = files.get(&r).and_then(Option::as_deref);
                list.into_iter()
                    .map(|(x, z)| {
                        let Some(f) = file else { return ((x, z), Load::Absent) };
                        ((x, z), read_one_shared(f, x, z, plan.read, data, states, h))
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Raw compounds of a region folder (`entities`, `poi`), one task per region.
    pub fn load_raw(&self, sub: &str, chunks: &[(i32, i32)]) -> Vec<((i32, i32), Option<rc_proto::nbt::Nbt>)> {
        self.load_raw_split(sub, chunks, usize::MAX)
    }

    /// Like [`Dimension::load_raw`], regions split as in [`Dimension::load_split`].
    pub fn load_raw_split(
        &self,
        sub: &str,
        chunks: &[(i32, i32)],
        per_task: usize,
    ) -> Vec<((i32, i32), Option<rc_proto::nbt::Nbt>)> {
        self.load_raw_split_with(sub, chunks, per_task, LoadPlan::from_switches())
    }

    pub(crate) fn load_raw_split_with(
        &self,
        sub: &str,
        chunks: &[(i32, i32)],
        per_task: usize,
        plan: LoadPlan,
    ) -> Vec<((i32, i32), Option<rc_proto::nbt::Nbt>)> {
        let dir = self.dir.join(sub);
        let tasks = tasks_by_region(chunks, per_task);
        if !plan.shared_handle {
            // Old path: every task opens its region file again.
            return tasks
                .into_par_iter()
                .flat_map_iter(|((rx, rz), list)| {
                    let mut file = open_for_reading(&dir.join(format!("r.{rx}.{rz}.mca")), plan.read);
                    list.into_iter()
                        .map(|(x, z)| {
                            let nbt = file
                                .as_mut()
                                .and_then(|f| f.read(local_index(x, z), x, z).ok().flatten())
                                .and_then(|b| crate::nbtio::parse_root(&b).ok());
                            ((x, z), nbt)
                        })
                        .collect::<Vec<_>>()
                })
                .collect();
        }
        let files = self.open_regions(&dir, &tasks, plan);
        tasks
            .into_par_iter()
            .flat_map_iter(|(r, list)| {
                let file = files.get(&r).and_then(Option::as_deref);
                list.into_iter()
                    .map(|(x, z)| {
                        let nbt = file
                            .and_then(|f| {
                                f.read_then(local_index(x, z), x, z, plan.read, crate::nbtio::parse_root).ok()
                            })
                            .flatten()
                            .and_then(Result::ok);
                        ((x, z), nbt)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Each region file of `tasks` opened once for this load, side by side on the pool; taken from
    /// (and left in) the dimension's kept files with `handle_cache`.
    fn open_regions(&self, folder: &Path, tasks: &[RegionTask], plan: LoadPlan) -> OpenRegions {
        let mut regions: Vec<(i32, i32)> = tasks.iter().map(|t| t.0).collect();
        regions.sort_unstable();
        regions.dedup();
        regions
            .into_par_iter()
            .map(|(rx, rz)| {
                let path = folder.join(format!("r.{rx}.{rz}.mca"));
                let file = if plan.handle_cache {
                    self.region_files.get(&path)
                } else {
                    SharedRegion::open(&path).ok().map(std::sync::Arc::new)
                };
                ((rx, rz), file)
            })
            .collect()
    }

    /// All chunks present in the dimension (tests, measurements).
    pub fn load_everything(&self, data: &VersionData, states: &States) -> Vec<((i32, i32), Load)> {
        self.load_everything_with(data, states, LoadPlan::from_switches())
    }

    pub(crate) fn load_everything_with(
        &self,
        data: &VersionData,
        states: &States,
        plan: LoadPlan,
    ) -> Vec<((i32, i32), Load)> {
        let dir = self.region_dir();
        let h = self.height;
        if !plan.shared_handle {
            // Old path: one task per region file.
            return self
                .regions()
                .into_par_iter()
                .flat_map_iter(|(rx, rz)| {
                    let path = dir.join(format!("r.{rx}.{rz}.mca"));
                    let Some(mut f) = open_for_reading(&path, plan.read) else { return Vec::new() };
                    let present: Vec<usize> = f.present().collect();
                    present
                        .into_iter()
                        .map(|i| {
                            let (x, z) = ((rx << 5) + (i & 31) as i32, (rz << 5) + (i >> 5) as i32);
                            ((x, z), read_one(&mut f, x, z, data, states, h))
                        })
                        .collect()
                })
                .collect();
        }
        // Every file opened once (side by side, freshly: the header lists what is present now),
        // then every chunk is its own piece of work: a full region no longer keeps one thread
        // busy while the others wait.
        let opened: Vec<((i32, i32), SharedRegion)> = self
            .regions()
            .into_par_iter()
            .filter_map(|(rx, rz)| Some(((rx, rz), SharedRegion::open(&dir.join(format!("r.{rx}.{rz}.mca"))).ok()?)))
            .collect();
        let work: Vec<(&SharedRegion, (i32, i32))> = opened
            .iter()
            .flat_map(|((rx, rz), f)| {
                f.present().map(move |i| (f, ((rx << 5) + (i & 31) as i32, (rz << 5) + (i >> 5) as i32)))
            })
            .collect();
        work.into_par_iter().map(|(f, (x, z))| ((x, z), read_one_shared(f, x, z, plan.read, data, states, h))).collect()
    }
}

/// Chunks of one region read by one pool task.
type RegionTask = ((i32, i32), Vec<(i32, i32)>);

/// Chunks grouped by region, each region cut into tasks of at most `per_task` chunks (in list
/// order). Listed first: a small hash table (a few regions) is not split between the threads
/// (a single group of slots), loading went region after region.
fn tasks_by_region(chunks: &[(i32, i32)], per_task: usize) -> Vec<RegionTask> {
    let mut by_region: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    for &(x, z) in chunks {
        by_region.entry((x >> 5, z >> 5)).or_default().push((x, z));
    }
    let per_task = per_task.max(1);
    by_region
        .into_iter()
        .flat_map(|(r, list)| {
            if list.len() <= per_task {
                vec![(r, list)]
            } else {
                list.chunks(per_task).map(|c| (r, c.to_vec())).collect()
            }
        })
        .collect()
}

/// A region file of its own for one task (old path), read the way `plan` says.
fn open_for_reading(path: &Path, plan: ReadPlan) -> Option<RegionFile> {
    let mut f = RegionFile::open(path, false).ok()?;
    f.read_plan = plan;
    Some(f)
}

fn read_one(f: &mut RegionFile, x: i32, z: i32, data: &VersionData, states: &States, h: Height) -> Load {
    match f.read(local_index(x, z), x, z) {
        Ok(Some(b)) => chunk_of(crate::nbtio::parse_root(&b), x, z, data, states, h),
        Ok(None) => Load::Absent,
        Err(e) => Load::Unreadable(e),
    }
}

/// Like `read_one` on a shared file: the NBT is parsed straight from the inflated bytes.
fn read_one_shared(
    f: &SharedRegion,
    x: i32,
    z: i32,
    plan: ReadPlan,
    data: &VersionData,
    states: &States,
    h: Height,
) -> Load {
    match f.read_then(local_index(x, z), x, z, plan, crate::nbtio::parse_root) {
        Ok(Some(root)) => chunk_of(root, x, z, data, states, h),
        Ok(None) => Load::Absent,
        Err(e) => Load::Unreadable(e),
    }
}

fn chunk_of(
    root: Result<rc_proto::nbt::Nbt, crate::Error>,
    x: i32,
    z: i32,
    data: &VersionData,
    states: &States,
    h: Height,
) -> Load {
    let root = match root {
        Ok(r) => r,
        Err(e) => return Load::Skipped(Skip::Corrupt(e.to_string())),
    };
    match chunk::parse(root, data, states, h) {
        Ok(c) if (c.x, c.z) == (x, z) => Load::Loaded(Box::new(c)),
        Ok(c) => Load::Skipped(Skip::Corrupt(format!("position ({}, {}) instead of ({x}, {z})", c.x, c.z))),
        Err(s) => Load::Skipped(s),
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
mod tests;
