//! Folder of a world and its dimensions (`anvil-world.md` § 0): chunk loading in
//! batches, regions read in parallel on the pool.

use hashbrown::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use rc_data::VersionData;

use crate::chunk::{self, Chunk, Height, Skip};
use crate::region::{RegionFile, Unreadable, local_index, region_name};
use crate::states::States;

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
}

/// Flags of a dimension type, same rules as the lookups they replace: unknown type
/// = sky, no ceiling, no "nether" lighting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DimKind {
    /// Sky light (first type of that name; absent: true).
    pub sky: bool,
    /// Ceiling (first type of that name; absent: false).
    pub ceiling: bool,
    /// Sky without "nether" lighting (first type of that name; absent: true): weather exists.
    pub weather: bool,
}

impl DimKind {
    pub(crate) fn of(data: &VersionData, name: &str) -> DimKind {
        let t = data.dimension_types.iter().find(|t| t.name == name);
        DimKind {
            sky: t.is_none_or(|t| t.has_skylight),
            ceiling: t.is_some_and(|t| t.has_ceiling),
            weather: t.is_none_or(|t| t.has_skylight && !t.nether_light),
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
        Dimension { name: name.to_owned(), dir: dimension_dir(world, name), height, kind: std::sync::OnceLock::new() }
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

    /// Loads chunks: grouped by region, one pool task per region.
    pub fn load(&self, chunks: &[(i32, i32)], data: &VersionData, states: &States) -> Vec<((i32, i32), Load)> {
        self.load_split(chunks, data, states, usize::MAX)
    }

    /// Like [`Dimension::load`], each region split into tasks of at most `per_task` chunks
    /// (each reopens the file): a small list taken from one or two regions then occupies
    /// several threads instead of one or two. Same outcomes, in the same order per region.
    pub fn load_split(
        &self,
        chunks: &[(i32, i32)],
        data: &VersionData,
        states: &States,
        per_task: usize,
    ) -> Vec<((i32, i32), Load)> {
        let mut by_region: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
        for &(x, z) in chunks {
            by_region.entry((x >> 5, z >> 5)).or_default().push((x, z));
        }
        let dir = self.region_dir();
        let h = self.height;
        // List first: a small hash table (a few regions) is not split between
        // the threads (a single group of slots), loading went region after region.
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
            .collect::<Vec<_>>()
            .into_par_iter()
            .flat_map_iter(|((rx, rz), list)| {
                let path = dir.join(region_name(rx << 5, rz << 5));
                let mut file = RegionFile::open(&path, false).ok();
                list.into_iter()
                    .map(|(x, z)| {
                        let Some(f) = file.as_mut() else { return ((x, z), Load::Absent) };
                        ((x, z), read_one(f, x, z, data, states, h))
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
        let mut by_region: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
        for &(x, z) in chunks {
            by_region.entry((x >> 5, z >> 5)).or_default().push((x, z));
        }
        let dir = self.dir.join(sub);
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
            .collect::<Vec<_>>()
            .into_par_iter()
            .flat_map_iter(|((rx, rz), list)| {
                let mut file = RegionFile::open(&dir.join(format!("r.{rx}.{rz}.mca")), false).ok();
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
            .collect()
    }

    /// All chunks present in the dimension (tests, measurements).
    pub fn load_everything(&self, data: &VersionData, states: &States) -> Vec<((i32, i32), Load)> {
        let dir = self.region_dir();
        let h = self.height;
        self.regions()
            .into_par_iter()
            .flat_map_iter(|(rx, rz)| {
                let path = dir.join(format!("r.{rx}.{rz}.mca"));
                let Ok(mut f) = RegionFile::open(&path, false) else { return Vec::new() };
                let present: Vec<usize> = f.present().collect();
                present
                    .into_iter()
                    .map(|i| {
                        let (x, z) = ((rx << 5) + (i & 31) as i32, (rz << 5) + (i >> 5) as i32);
                        ((x, z), read_one(&mut f, x, z, data, states, h))
                    })
                    .collect()
            })
            .collect()
    }
}

fn read_one(f: &mut RegionFile, x: i32, z: i32, data: &VersionData, states: &States, h: Height) -> Load {
    let bytes = match f.read(local_index(x, z), x, z) {
        Ok(Some(b)) => b,
        Ok(None) => return Load::Absent,
        Err(e) => return Load::Unreadable(e),
    };
    let root = match crate::nbtio::parse_root(&bytes) {
        Ok(r) => r,
        Err(e) => return Load::Skipped(Skip::Corrupt(e.to_string())),
    };
    match chunk::parse(root, data, states, h) {
        Ok(c) if (c.x, c.z) == (x, z) => Load::Loaded(Box::new(c)),
        Ok(c) => Load::Skipped(Skip::Corrupt(format!("position ({}, {}) instead of ({x}, {z})", c.x, c.z))),
        Err(s) => Load::Skipped(s),
    }
}
