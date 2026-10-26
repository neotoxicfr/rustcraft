//! Saves (`integrated-server.md` § 8, `anvil-world.md` § 7–8): snapshot of the modified data
//! taken on the server thread, compression and writes on a write thread, in the game's
//! order (players → chunks → `level.dat` → `data/`).

use hashbrown::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use rc_proto::nbt::Nbt;

use crate::nbtio::{self, set};
use crate::region::{RegionFile, local_index, region_name};
use crate::server::Server;

/// Write job.
pub(crate) enum Job {
    /// Chunk (uncompressed NBT) in its region.
    Chunk { path: PathBuf, cx: i32, cz: i32, nbt: Vec<u8> },
    /// `.dat` file (uncompressed NBT, gzip when writing), safe replacement; `backup`:
    /// keep the old one as `<name>_old`.
    Dat { path: PathBuf, nbt: Vec<u8>, backup: bool },
    /// Text file written as is (advancements, statistics: JSON), S9.
    Raw { path: PathBuf, bytes: Vec<u8> },
    /// End of a full save: writes forced to disk.
    Sync,
    /// Save finished: "in progress" flag reset to false.
    Done(std::sync::Arc<std::sync::atomic::AtomicBool>),
    /// Previous writes done: the counter goes to at least this number (acknowledgements returned to
    /// the generation thread, `generation.rs`).
    Ack(std::sync::Arc<std::sync::atomic::AtomicU64>, u64),
}

pub(crate) struct Writer {
    tx: Option<crossbeam_channel::Sender<Vec<Job>>>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(test)]
thread_local! {
    /// Tests: latency (ms) added before each batch by the writer created from this thread.
    pub(crate) static TEST_WRITER_DELAY_MS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Latency injected before each write batch (`RUSTCRAFT_TEST_WRITER_DELAY_MS`, read once;
/// robustness trials only, 0 by default).
fn writer_delay() -> std::time::Duration {
    static ENV: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let env = *ENV.get_or_init(|| {
        rc_data::env_str!("RUSTCRAFT_TEST_WRITER_DELAY_MS").and_then(|v| v.parse().ok()).unwrap_or(0)
    });
    #[cfg(test)]
    let env = env.max(TEST_WRITER_DELAY_MS.with(std::cell::Cell::get));
    std::time::Duration::from_millis(env)
}

impl Writer {
    pub(crate) fn new() -> Writer {
        let (tx, rx) = crossbeam_channel::unbounded::<Vec<Job>>();
        let delay = writer_delay();
        let thread = std::thread::Builder::new()
            .name("solo-write".into())
            .spawn(move || {
                let mut regions: HashMap<PathBuf, RegionFile> = HashMap::new();
                for jobs in rx {
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    for j in jobs {
                        if let Err(e) = run(&mut regions, j) {
                            eprintln!("singleplayer: write: {e}");
                        }
                    }
                }
                // Closing: regions padded and forced to disk.
                for (path, r) in regions {
                    if let Err(e) = r.close() {
                        eprintln!("singleplayer: closing {}: {e}", path.display());
                    }
                }
            })
            .ok();
        Writer { tx: Some(tx), thread }
    }

    pub(crate) fn send(&self, jobs: Vec<Job>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(jobs);
        }
    }

    /// Batches waiting (memory sampling).
    pub(crate) fn queued(&self) -> usize {
        self.tx.as_ref().map_or(0, crossbeam_channel::Sender::len)
    }

    /// Waits for the end of all writes and closes the regions.
    pub(crate) fn finish(self) {
        drop(self);
    }
}

/// Dropped without `finish` (server forgotten without closing): queued writes are awaited too, so that
/// none lands afterwards in an erased or reopened folder (new world at the same path).
impl Drop for Writer {
    fn drop(&mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(regions: &mut HashMap<PathBuf, RegionFile>, job: Job) -> std::io::Result<()> {
    match job {
        Job::Chunk { path, cx, cz, nbt } => {
            let r = match regions.entry(path.clone()) {
                hashbrown::hash_map::Entry::Occupied(e) => e.into_mut(),
                hashbrown::hash_map::Entry::Vacant(e) => e.insert(RegionFile::open_or_create(&path)?),
            };
            r.write(local_index(cx, cz), cx, cz, &nbt)
        }
        Job::Dat { path, nbt, backup } => replace(&path, &nbtio::gzip(&nbt), backup),
        Job::Raw { path, bytes } => replace(&path, &bytes, false),
        Job::Sync => {
            for r in regions.values_mut() {
                r.sync()?;
            }
            Ok(())
        }
        Job::Done(flag) => {
            flag.store(false, std::sync::atomic::Ordering::Release);
            Ok(())
        }
        Job::Ack(counter, n) => {
            counter.fetch_max(n, std::sync::atomic::Ordering::Release);
            Ok(())
        }
    }
}

/// Retries up to 10 times (files held by an antivirus on Windows).
fn retry(mut f: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut last = Ok(());
    for i in 0..10 {
        last = f();
        if last.is_ok() {
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(10 * (i + 1)));
    }
    last
}

/// Safe replacement (§ 8.2): temporary forced to disk, old copy as a backup, rename.
pub(crate) fn replace(path: &Path, bytes: &[u8], backup: bool) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let nonce = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!("{stem}-{nonce:x}.dat"));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
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

fn doubles(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&d| Nbt::Double(d)).collect())
}

impl Server {
    /// Player file: compound read, managed fields replaced (§ 5).
    pub(crate) fn player_nbt(&self) -> Nbt {
        let mut root = self.pdata.root.clone();
        set(&mut root, "DataVersion", Nbt::Int(crate::chunk::DATA_VERSION));
        set(&mut root, "Pos", doubles(self.pos));
        set(&mut root, "Rotation", Nbt::List(vec![Nbt::Float(self.yaw), Nbt::Float(self.pitch)]));
        set(&mut root, "OnGround", Nbt::Byte(i8::from(self.on_ground)));
        set(&mut root, "Dimension", Nbt::String(self.dim.name.clone()));
        set(&mut root, "playerGameType", Nbt::Int(self.game_mode));
        set(&mut root, "SelectedItemSlot", Nbt::Int(self.held));
        let mut inv = Vec::new();
        for disk in 0..36 {
            if let Some(menu) = crate::player::menu_slot(disk)
                && let Some(s) = &self.slots[menu]
            {
                inv.push(s.to_nbt(Some(disk as i8)));
            }
        }
        set(&mut root, "Inventory", Nbt::List(inv));
        let mut eq = root.get("equipment").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for (k, i) in [("head", 5), ("chest", 6), ("legs", 7), ("feet", 8), ("offhand", 45)] {
            match &self.slots[i] {
                Some(s) => set(&mut eq, k, s.to_nbt(None)),
                None => nbtio::remove(&mut eq, k),
            }
        }
        if matches!(&eq, Nbt::Compound(f) if !f.is_empty()) {
            set(&mut root, "equipment", eq);
        } else {
            nbtio::remove(&mut root, "equipment");
        }
        let mut ab = root.get("abilities").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        let a = self.abilities();
        for (k, v) in [
            ("flying", a.flying),
            ("mayfly", a.may_fly),
            ("instabuild", a.instabuild),
            ("invulnerable", a.invulnerable),
            ("mayBuild", self.game_mode != 2 && self.game_mode != 3),
        ] {
            set(&mut ab, k, Nbt::Byte(i8::from(v)));
        }
        set(&mut ab, "flySpeed", Nbt::Float(a.flying_speed));
        set(&mut ab, "walkSpeed", Nbt::Float(a.walking_speed));
        set(&mut root, "abilities", ab);
        self.life_player_nbt(&mut root); // S2: health, hunger, experience, book, Ender
        self.dims_player_nbt(&mut root); // S6: portal delay, credits seen
        self.proj_player_nbt(&mut root); // S8: ender pearls
        crate::mobs::surface::parrot::save_shoulders(self, &mut root); // F02: parrots
        self.mount_player_nbt(&mut root); // F04: ridden vehicle
        root
    }

    /// `level.dat`: compound read, `Time`, `LastPlayed`, `singleplayer_uuid`, version history
    /// updated (§ 2); nothing else changes (neither `ServerBrands` nor `WasModded`).
    fn level_nbt(&self) -> Nbt {
        let mut root = self.level.root.clone();
        let Some(d) = nbtio::get_mut(&mut root, "Data") else { return root };
        set(d, "Time", Nbt::Long(self.time));
        let now = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
        set(d, "LastPlayed", Nbt::Long(now));
        set(d, "singleplayer_uuid", nbtio::uuid_nbt(self.uuid));
        let v = crate::chunk::DATA_VERSION;
        match nbtio::get_mut(d, "version_history") {
            Some(Nbt::List(l)) => {
                if !l.iter().any(|x| x.as_i64() == Some(i64::from(v))) {
                    l.push(Nbt::Int(v));
                }
            }
            _ => set(d, "version_history", Nbt::List(vec![Nbt::Int(v)])),
        }
        root
    }

    /// `world_clocks.dat` with the current clocks (only if they advanced).
    fn clocks_nbt(&self) -> Option<Nbt> {
        if self.clocks == self.clocks_loaded {
            return None;
        }
        let path = self.world.join("data").join("minecraft").join("world_clocks.dat");
        let mut root = nbtio::read_dat(&path).unwrap_or_else(|_| Nbt::Compound(Vec::new()));
        let mut d = root.get("data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
        for (name, total) in &self.clocks {
            let mut c = d.get(name).cloned().unwrap_or(Nbt::Compound(Vec::new()));
            set(&mut c, "total_ticks", Nbt::Long(*total));
            set(&mut d, name, c);
        }
        set(&mut root, "data", d);
        set(&mut root, "DataVersion", Nbt::Int(crate::chunk::DATA_VERSION));
        Some(root)
    }

    /// Modified chunks of the active dimension: write jobs; returns their count.
    pub(crate) fn chunk_jobs(&mut self, jobs: &mut Vec<Job>) -> usize {
        let region_dir = self.dim.region_dir();
        let h = self.dim.height;
        let mut written = 0;
        let mut dirty: Vec<(i32, i32)> =
            self.chunks.iter().filter(|(_, c)| c.as_ref().is_some_and(|c| c.dirty)).map(|(p, _)| *p).collect();
        dirty.sort_unstable();
        for p in dirty {
            self.export_chunk_ticks(p);
            let Some(Some(c)) = self.chunks.get_mut(&p) else { continue };
            c.dirty = false;
            let Some(root) = c.encode(&self.data, &self.states, h, self.time) else {
                eprintln!("singleplayer: chunk {p:?} not rewritten (unknown identifier)");
                continue;
            };
            c.changed.iter_mut().for_each(|x| *x = false);
            let nbt = nbtio::encode_root(&root);
            c.root = root;
            jobs.push(Job::Chunk { path: region_dir.join(region_name(p.0, p.1)), cx: p.0, cz: p.1, nbt });
            written += 1;
        }
        // Full chunks received from the generator up to batch `seen`: written above or before.
        if let Some(g) = &self.generator {
            jobs.push(Job::Ack(g.saved.clone(), g.seen));
        }
        written
    }

    /// Full save (§ 8). `wait`: closing, writes awaited and regions closed.
    pub(crate) fn save(&mut self, wait: bool) {
        let t = std::time::Instant::now();
        self.saving.store(true, std::sync::atomic::Ordering::Release);
        // Proto-chunks of the generator not yet written: in the queue before everything else.
        self.flush_gen(wait);
        let mut jobs = Vec::new();
        self.life_save_jobs(&mut jobs); // S2: furnaces, random sequences, weather
        self.commands_save_jobs(&mut jobs); // S5: rules, scores, storage, forced chunks
        self.maps_save_jobs(&mut jobs); // S8: maps
        self.progress_save_jobs(&mut jobs); // S9: advancements, statistics (including "time since rest")
        self.lan_save_jobs(&mut jobs); // S9: LAN players
        self.raids_save_jobs(&mut jobs); // F07: raids.dat
        self.end_save_jobs(&mut jobs); // F15: dragon fight
        let player_path = self.pdata.path.clone();
        jobs.push(Job::Dat { path: player_path, nbt: nbtio::encode_root(&self.player_nbt()), backup: true });
        let mut written = self.chunk_jobs(&mut jobs);
        written += self.save_parked(&mut jobs, wait); // S6: other dimensions
        jobs.push(Job::Dat {
            path: self.world.join("level.dat"),
            nbt: nbtio::encode_root(&self.level_nbt()),
            backup: true,
        });
        if let Some(c) = self.clocks_nbt() {
            let path = self.world.join("data").join("minecraft").join("world_clocks.dat");
            jobs.push(Job::Dat { path, nbt: nbtio::encode_root(&c), backup: false });
            self.clocks_loaded = self.clocks.clone();
        }
        jobs.push(Job::Sync);
        jobs.push(Job::Done(self.saving.clone()));
        let snapshot = t.elapsed();
        let writer = self.writer.get_or_insert_with(Writer::new);
        writer.send(jobs);
        if wait && let Some(w) = self.writer.take() {
            w.finish();
        }
        eprintln!(
            "singleplayer: {}save: {written} chunks, snapshot {:.2} ms{}",
            if wait { "closing " } else { "" },
            snapshot.as_secs_f64() * 1e3,
            if wait { format!(", total {:.1} ms", t.elapsed().as_secs_f64() * 1e3) } else { String::new() }
        );
    }
}

#[cfg(test)]
mod writer_tests {
    use super::*;

    /// Writer dropped without `finish` (server forgotten without closing, like the simulations of the
    /// tests): its queued writes are done before the drop returns control. Otherwise they fell later
    /// into the folder, possibly into the next new world at the same path.
    #[test]
    fn drop_waits_for_queued_writes() {
        TEST_WRITER_DELAY_MS.with(|d| d.set(200));
        let w = Writer::new();
        TEST_WRITER_DELAY_MS.with(|d| d.set(0));
        let n = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        w.send(vec![Job::Ack(n.clone(), 7)]);
        drop(w);
        assert_eq!(n.load(std::sync::atomic::Ordering::Acquire), 7, "writes still in progress after the drop");
    }
}
