//! The save's write thread: chunk compression, region writes in the batch order, `.dat`
//! replacements, forced writes (`anvil-world.md` § 3.3, § 8.2).

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hashbrown::HashMap;
use hashbrown::hash_map::Entry;
use rayon::prelude::*;

use super::{Job, save_pool, timings_enabled};
use crate::nbtio;
use crate::region::{RegionFile, local_index};

/// How the write thread works (`RUSTCRAFT_OPT_*`, read when the writer starts; tests and benches
/// build their own). Every plan leaves the same files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WriterPlan {
    /// The chunks of a batch compressed side by side on the save pool, then written one by one in
    /// the batch's order, so that sectors are allocated as before
    /// (`RUSTCRAFT_OPT_PARALLEL_SAVE_COMPRESSION`).
    pub parallel_compression: bool,
    /// A sync or a closing forces only the regions written since their last forced write
    /// (`RUSTCRAFT_OPT_SKIP_CLEAN_REGION_SYNC`): after a long flight, hundreds of untouched files.
    pub skip_clean_regions: bool,
    /// A region is padded before its forced write, so that closing it afterwards forces nothing
    /// more: one forced write per region at closing instead of two
    /// (`RUSTCRAFT_OPT_PAD_REGIONS_BEFORE_SYNC`).
    pub pad_before_sync: bool,
    /// Region syncs, region closings and consecutive file replacements run side by side on the
    /// save pool: their waits on the disk overlap (`RUSTCRAFT_OPT_PARALLEL_FILE_SYNC`).
    pub parallel_file_sync: bool,
    /// Region files kept open per folder; beyond, the one written longest ago is closed, like the
    /// game's cache of 256 (`RUSTCRAFT_OPT_BOUNDED_OPEN_REGIONS`). `None`: every region stays open
    /// until the end (old path).
    pub max_open_per_folder: Option<usize>,
    /// Compressor of the chunk streams (`RUSTCRAFT_OPT_SAVE_LIBDEFLATE`).
    pub compressor: nbtio::ZlibCompressor,
    /// Tests: what the proofs break on purpose.
    #[cfg(test)]
    pub sabotage: WriterSabotage,
}

/// Deliberate faults of the write thread, to check that the proofs see them.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriterSabotage {
    None,
    /// The first two chunks of a batch written in the other order.
    SwapFirstTwoChunks,
    /// Every open region forced at a sync, written or not.
    SyncCleanRegions,
    /// The first region written since the last sync left out of the next one.
    ForgetOneWrittenRegion,
}

/// The game keeps up to 256 region files open per folder.
const MAX_OPEN_REGIONS_PER_FOLDER: usize = 256;

impl WriterPlan {
    /// The old path: compression and forced writes one after the other, every region forced at
    /// every sync and again at closing, regions never closed before the end.
    #[cfg(test)]
    pub(crate) const OLD: WriterPlan = WriterPlan {
        parallel_compression: false,
        skip_clean_regions: false,
        pad_before_sync: false,
        parallel_file_sync: false,
        max_open_per_folder: None,
        compressor: nbtio::ZlibCompressor::Flate2,
        sabotage: WriterSabotage::None,
    };

    pub(crate) fn from_switches() -> WriterPlan {
        WriterPlan {
            parallel_compression: rc_data::opt_switch!("RUSTCRAFT_OPT_PARALLEL_SAVE_COMPRESSION"),
            skip_clean_regions: skip_clean_region_sync(),
            pad_before_sync: rc_data::opt_switch!("RUSTCRAFT_OPT_PAD_REGIONS_BEFORE_SYNC"),
            parallel_file_sync: rc_data::opt_switch!("RUSTCRAFT_OPT_PARALLEL_FILE_SYNC"),
            max_open_per_folder: rc_data::opt_switch!("RUSTCRAFT_OPT_BOUNDED_OPEN_REGIONS")
                .then_some(MAX_OPEN_REGIONS_PER_FOLDER),
            compressor: nbtio::save_compressor(),
            #[cfg(test)]
            sabotage: WriterSabotage::None,
        }
    }
}

/// See [`WriterPlan::skip_clean_regions`]; also read by the world optimisation screen.
pub(crate) fn skip_clean_region_sync() -> bool {
    rc_data::opt_switch!("RUSTCRAFT_OPT_SKIP_CLEAN_REGION_SYNC")
}

/// The write thread's state from batch to batch.
pub(crate) struct WriteThread {
    plan: WriterPlan,
    regions: OpenRegions,
    stats: WriteStats,
    /// Tests: the regions each sync forced.
    #[cfg(test)]
    pub(crate) synced: Vec<Vec<PathBuf>>,
}

impl WriteThread {
    pub(crate) fn new(plan: WriterPlan) -> WriteThread {
        WriteThread {
            plan,
            regions: OpenRegions::default(),
            stats: WriteStats::starting_now(),
            #[cfg(test)]
            synced: Vec::new(),
        }
    }

    /// Runs one batch in its order. The chunk streams may be compressed beforehand side by side;
    /// consecutive file replacements may run side by side.
    pub(crate) fn run_batch(&mut self, jobs: Vec<Job>) {
        #[cfg(test)]
        let jobs = self.sabotaged(jobs);
        let streams = self.compress_chunks(&jobs);
        let mut jobs = jobs.into_iter().zip(streams).peekable();
        while let Some((job, stream)) = jobs.next() {
            if self.plan.parallel_file_sync && job.replaced_file().is_some() {
                let mut group = vec![job];
                // Up to the next job of another kind, or the same file twice (kept in order).
                while let Some((next, _)) = jobs.peek() {
                    let Some(path) = next.replaced_file() else { break };
                    if group.iter().any(|g| g.replaced_file() == Some(path)) {
                        break;
                    }
                    if let Some((next, _)) = jobs.next() {
                        group.push(next);
                    }
                }
                self.replace_files(group);
                continue;
            }
            if let Err(e) = self.run(job, stream) {
                eprintln!("singleplayer: write: {e}");
            }
        }
    }

    /// End of the thread: every region padded and forced to disk (only the ones written since
    /// their last forced write with `skip_clean_regions`).
    pub(crate) fn finish(self) {
        let t = Instant::now();
        let regions: Vec<(PathBuf, RegionFile)> = self.regions.files.into_iter().map(|(p, o)| (p, o.region)).collect();
        let count = regions.len();
        let only_if_written = self.plan.skip_clean_regions;
        let closed = on_files(self.plan.parallel_file_sync, regions, |(path, r)| (path, r.close_with(only_if_written)));
        let mut forced = 0;
        for (path, result) in closed {
            match result {
                Ok(f) => forced += usize::from(f),
                Err(e) => eprintln!("singleplayer: closing {}: {e}", path.display()),
            }
        }
        if timings_enabled() {
            eprintln!(
                "singleplayer: write thread closing: {count} regions, {forced} forced, {:.1} ms",
                t.elapsed().as_secs_f64() * 1e3
            );
        }
    }

    fn run(&mut self, job: Job, stream: Option<Vec<u8>>) -> io::Result<()> {
        match job {
            Job::Chunk { path, cx, cz, nbt } => {
                let stream = stream.unwrap_or_else(|| {
                    let t = Instant::now();
                    let s = nbtio::zlib_with(&nbt, self.plan.compressor);
                    self.stats.compression += t.elapsed();
                    s
                });
                let t = Instant::now();
                let region = self.regions.get(&path, &self.plan)?;
                let result = region.write_compressed(local_index(cx, cz), cx, cz, &stream);
                self.stats.chunks += 1;
                self.stats.region_writes += t.elapsed();
                result
            }
            Job::Dat { .. } | Job::Raw { .. } => {
                let t = Instant::now();
                let result = replace_file(job);
                self.stats.files += 1;
                self.stats.file_replacements += t.elapsed();
                result
            }
            Job::Sync => {
                self.sync_regions();
                Ok(())
            }
            Job::Done(flag) => {
                self.stats.report(self.regions.files.len());
                self.stats = WriteStats::starting_now();
                flag.store(false, std::sync::atomic::Ordering::Release);
                Ok(())
            }
            Job::Ack(counter, n) => {
                counter.fetch_max(n, std::sync::atomic::Ordering::Release);
                Ok(())
            }
        }
    }

    /// zlib streams of the batch's chunks, compressed side by side; `None` for the other jobs and
    /// on the serial path (each chunk is then compressed right before its write).
    fn compress_chunks(&mut self, jobs: &[Job]) -> Vec<Option<Vec<u8>>> {
        let chunks = jobs.iter().filter(|j| matches!(j, Job::Chunk { .. })).count();
        let pool = save_pool().filter(|_| self.plan.parallel_compression && chunks > 1);
        let Some(pool) = pool else { return jobs.iter().map(|_| None).collect() };
        let t = Instant::now();
        let streams = compress_chunks_on(pool, jobs, self.plan.compressor);
        self.stats.compression += t.elapsed();
        streams
    }

    fn replace_files(&mut self, group: Vec<Job>) {
        let t = Instant::now();
        self.stats.files += group.len();
        for e in on_files(true, group, replace_file).into_iter().filter_map(Result::err) {
            eprintln!("singleplayer: write: {e}");
        }
        self.stats.file_replacements += t.elapsed();
    }

    /// `Job::Sync`: the open regions forced to disk.
    fn sync_regions(&mut self) {
        let t = Instant::now();
        #[cfg(test)]
        self.sabotage_before_sync();
        let plan = self.plan;
        #[cfg(test)]
        let only_if_written = plan.skip_clean_regions && plan.sabotage != WriterSabotage::SyncCleanRegions;
        #[cfg(not(test))]
        let only_if_written = plan.skip_clean_regions;
        let regions: Vec<(&PathBuf, &mut RegionFile)> =
            self.regions.files.iter_mut().map(|(p, o)| (p, &mut o.region)).collect();
        let results = on_files(plan.parallel_file_sync, regions, |(path, r)| {
            (path, r.sync_with(plan.pad_before_sync, only_if_written))
        });
        #[cfg(test)]
        let mut synced = Vec::new();
        for (path, result) in results {
            match result {
                Ok(true) => {
                    self.stats.forced_regions += 1;
                    #[cfg(test)]
                    synced.push(path.clone());
                }
                Ok(false) => {}
                Err(e) => eprintln!("singleplayer: write: {}: {e}", path.display()),
            }
        }
        #[cfg(test)]
        {
            synced.sort();
            self.synced.push(synced);
        }
        self.stats.region_syncs += t.elapsed();
    }

    #[cfg(test)]
    fn sabotaged(&self, mut jobs: Vec<Job>) -> Vec<Job> {
        if self.plan.sabotage == WriterSabotage::SwapFirstTwoChunks {
            let mut chunks = jobs.iter().enumerate().filter(|(_, j)| matches!(j, Job::Chunk { .. })).map(|(i, _)| i);
            if let (Some(a), Some(b)) = (chunks.next(), chunks.next()) {
                jobs.swap(a, b);
            }
        }
        jobs
    }

    #[cfg(test)]
    fn sabotage_before_sync(&mut self) {
        if self.plan.sabotage != WriterSabotage::ForgetOneWrittenRegion {
            return;
        }
        let mut written: Vec<(&PathBuf, &mut OpenRegion)> =
            self.regions.files.iter_mut().filter(|(_, o)| o.region.written_since_sync()).collect();
        written.sort_by(|a, b| a.0.cmp(b.0));
        if let Some((_, o)) = written.into_iter().next() {
            o.region.forget_writes();
        }
    }
}

/// zlib streams of the chunks of `jobs` (`None` for the other jobs), compressed side by side on
/// `pool`, in the jobs' order.
pub(crate) fn compress_chunks_on(
    pool: &rayon::ThreadPool,
    jobs: &[Job],
    compressor: nbtio::ZlibCompressor,
) -> Vec<Option<Vec<u8>>> {
    pool.install(|| {
        jobs.par_iter()
            .map(|j| match j {
                Job::Chunk { nbt, .. } => Some(nbtio::zlib_with(nbt, compressor)),
                _ => None,
            })
            .collect()
    })
}

/// `f` on every item; side by side on the save pool when `parallel` (forced writes and file
/// replacements spend their time waiting on the disk, the waits then overlap).
fn on_files<T: Send, R: Send>(parallel: bool, items: Vec<T>, f: impl Fn(T) -> R + Sync + Send) -> Vec<R> {
    match save_pool() {
        Some(pool) if parallel && items.len() > 1 => pool.install(|| items.into_par_iter().map(f).collect()),
        _ => items.into_iter().map(f).collect(),
    }
}

/// A `.dat` or a raw file replaced; any other job does nothing.
fn replace_file(job: Job) -> io::Result<()> {
    match job {
        Job::Dat { path, nbt, backup } => replace(&path, &nbtio::gzip(&nbt), backup),
        Job::Raw { path, bytes } => replace(&path, &bytes, false),
        _ => Ok(()),
    }
}

/// Region files the write thread keeps open, with the order of their last write.
#[derive(Default)]
struct OpenRegions {
    files: HashMap<PathBuf, OpenRegion>,
    /// Chunks written so far: the clock of `last_write`.
    writes: u64,
}

struct OpenRegion {
    region: RegionFile,
    last_write: u64,
}

impl OpenRegions {
    /// The region at `path`, opened (created if needed) at its first write. With a bound per
    /// folder, the folder's least recently written region is closed first to make room.
    fn get(&mut self, path: &Path, plan: &WriterPlan) -> io::Result<&mut RegionFile> {
        self.writes += 1;
        if let Some(max) = plan.max_open_per_folder
            && !self.files.contains_key(path)
        {
            self.close_least_recent(path.parent(), max.saturating_sub(1), plan.skip_clean_regions);
        }
        let open = match self.files.entry(path.to_owned()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(OpenRegion { region: RegionFile::open_or_create(path)?, last_write: 0 }),
        };
        open.last_write = self.writes;
        Ok(&mut open.region)
    }

    /// Closes the least recently written regions of `folder` until at most `keep` stay open.
    fn close_least_recent(&mut self, folder: Option<&Path>, keep: usize, only_if_written: bool) {
        loop {
            let in_folder = self.files.iter().filter(|(p, _)| p.parent() == folder);
            let (count, oldest) = in_folder.fold((0, None::<(&PathBuf, u64)>), |(n, old), (p, o)| {
                let older = old.is_none_or(|(_, t)| o.last_write < t);
                (n + 1, if older { Some((p, o.last_write)) } else { old })
            });
            let Some((path, _)) = oldest.filter(|_| count > keep) else { return };
            let path = path.clone();
            if let Some(open) = self.files.remove(&path)
                && let Err(e) = open.region.close_with(only_if_written)
            {
                eprintln!("singleplayer: closing {}: {e}", path.display());
            }
        }
    }
}

/// What the write thread did since the end of the previous save (save timings).
struct WriteStats {
    chunks: usize,
    compression: Duration,
    region_writes: Duration,
    files: usize,
    file_replacements: Duration,
    forced_regions: usize,
    region_syncs: Duration,
    /// Forced writes of the process when the count started.
    forced_writes_before: u64,
}

impl WriteStats {
    fn starting_now() -> WriteStats {
        WriteStats {
            chunks: 0,
            compression: Duration::ZERO,
            region_writes: Duration::ZERO,
            files: 0,
            file_replacements: Duration::ZERO,
            forced_regions: 0,
            region_syncs: Duration::ZERO,
            forced_writes_before: crate::region::file_syncs(),
        }
    }

    fn report(&self, open_regions: usize) {
        if !timings_enabled() {
            return;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let steps = crate::region::take_write_times();
        eprintln!(
            "singleplayer: write thread since the previous save: {} chunks, compression {:.1} ms, region writes {:.1} ms (allocate {:.1}, \
             sectors {:.1}, header {:.1}, external check {:.1}), {} files replaced {:.1} ms, {} regions forced \
             {:.1} ms, {} forced writes, {open_regions} regions open",
            self.chunks,
            ms(self.compression),
            ms(self.region_writes),
            ms(steps.allocate),
            ms(steps.sectors),
            ms(steps.header),
            ms(steps.external_check),
            self.files,
            ms(self.file_replacements),
            self.forced_regions,
            ms(self.region_syncs),
            crate::region::file_syncs() - self.forced_writes_before,
        );
    }
}

/// Retries up to 10 times (files held by an antivirus on Windows).
fn retry(mut f: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut last = Ok(());
    for i in 0..10 {
        last = f();
        if last.is_ok() {
            return last;
        }
        std::thread::sleep(Duration::from_millis(10 * (i + 1)));
    }
    last
}

#[cfg(test)]
/// Tests: files whose replacement stops between the forced temporary and the first rename, as a
/// crash would.
pub(crate) static CRASH_BEFORE_RENAME: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Temporary next to `path`, created new: two replacements running side by side never share one.
fn create_temporary(dir: &Path, stem: &str) -> io::Result<(PathBuf, std::fs::File)> {
    let mut nonce = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.subsec_nanos());
    loop {
        let tmp = dir.join(format!("{stem}-{nonce:x}.dat"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => return Ok((tmp, f)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => nonce = nonce.wrapping_add(1),
            Err(e) => return Err(e),
        }
    }
}

/// Safe replacement (§ 8.2): temporary forced to disk, old copy as a backup, rename.
pub(crate) fn replace(path: &Path, bytes: &[u8], backup: bool) -> io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (tmp, mut f) = create_temporary(dir, &stem)?;
    f.write_all(bytes)?;
    crate::region::sync_file(&f)?;
    drop(f);
    #[cfg(test)]
    if CRASH_BEFORE_RENAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().any(|p| p == path) {
        return Err(io::Error::other("simulated crash before the rename"));
    }
    let old =
        dir.join(format!("{}_old", path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()));
    if path.exists() && backup {
        if old.exists() {
            retry(|| std::fs::remove_file(&old))?;
        }
        retry(|| std::fs::rename(path, &old))?;
    }
    if path.exists() {
        retry(|| std::fs::remove_file(path))?;
    }
    if let Err(e) = retry(|| std::fs::rename(&tmp, path)) {
        if backup && old.exists() {
            let _ = std::fs::copy(&old, path);
        }
        return Err(e);
    }
    Ok(())
}
