//! World folders as seen by the "Singleplayer" screens (`anvil-world.md`): free folder name,
//! rewriting of `level.dat` (name, data packs), zip backup copy in `backups`,
//! restoration of `level.dat_old`, deletion and optimisation (chunk rewriting).

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

use rc_proto::Reader;
use rc_proto::nbt::{self, Nbt};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::Error;
use crate::lock::SessionLock;
use crate::nbtio::{self, set};
use crate::region::RegionFile;

/// Characters refused in a folder name (replaced by `_`), like the game.
const ILLEGAL: [char; 15] = ['/', '\n', '\r', '\t', '\0', '\u{c}', '`', '?', '*', '\\', '<', '>', '|', '"', ':'];

/// Cleaned folder name: forbidden characters and `.`, `/`, `"` replaced by `_`.
pub fn sanitize_name(base: &str) -> String {
    base.chars().map(|c| if ILLEGAL.contains(&c) || matches!(c, '.' | '/' | '"') { '_' } else { c }).collect()
}

/// Reserved Windows name (`CON`, `COM1`…, or ending with a dot): refused as is. The game's list
/// also holds a bare `COM` (but no bare `LPT`).
fn reserved(name: &str) -> bool {
    if name.ends_with('.') {
        return true;
    }
    let up = name.to_ascii_uppercase();
    let stem = up.split('.').next().unwrap_or("");
    matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "COM")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// First free name in `dir` for `base` (+ `suffix`): cleaned name, then " (n)" added from 1
/// as long as the folder exists (case ignored on Windows). A base already ending in " (n)" keeps
/// it and gets a second counter ("World (3)" → "World (3) (1)"): the game's copy-counter
/// pattern never matches (quirk reproduced, `docs/quirks-to-reproduce.md`).
pub fn find_available_name(dir: &Path, base: &str, suffix: &str) -> String {
    let mut base = sanitize_name(base);
    if reserved(&base) {
        base = format!("_{base}_");
    }
    let mut count = 0u32;
    let limit = |s: &str, n: usize| -> String { s.chars().take(n).collect() };
    let max = 255usize.saturating_sub(suffix.chars().count());
    base = limit(&base, max);
    let taken: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().to_lowercase()).collect())
        .unwrap_or_default();
    loop {
        let mut name = base.clone();
        if count != 0 {
            let tail = format!(" ({count})");
            name = limit(&name, 255 - tail.chars().count());
            name.push_str(&tail);
        }
        name.push_str(suffix);
        if !taken.contains(&name.to_lowercase()) && !dir.join(&name).exists() {
            return name;
        }
        count += 1;
    }
}

/// Root of `level.dat` (`old`: `level.dat_old`), without fallback.
pub fn read_level(dir: &Path, old: bool) -> Result<Nbt, Error> {
    nbtio::read_dat(&dir.join(if old { "level.dat_old" } else { "level.dat" }))
}

/// Root of `level.dat` as the world list reads it, like the game: the player (`Data.Player`) and
/// the old generation settings (`Data.WorldGenSettings`) are skipped without being decoded, so
/// damage in them no longer turns the world into an unreadable one (26.3 writes neither; older
/// worlds do).
pub fn read_level_summary(dir: &Path) -> Result<Nbt, Error> {
    let path = dir.join("level.dat");
    let raw = std::fs::read(&path).map_err(|e| Error::io(&path, e))?;
    let bytes = nbtio::decompress_auto(&raw).map_err(|e| Error::io(&path, e))?;
    match nbt::read_named_skipping(&mut Reader::new(&bytes), "Data", &["Player", "WorldGenSettings"]) {
        Ok(Some((_, root @ Nbt::Compound(_)))) => Ok(root),
        Ok(_) => Err(Error::Format(format!("{} : NBT root is not a compound", path.display()))),
        Err(e) => Err(Error::Format(format!("{} : NBT : {e}", path.display()))),
    }
}

/// Threads for the folder work spread over the cores (optimisation, backup copy, deletion):
/// `RUSTCRAFT_THREADS`, otherwise the logical cores.
fn worker_threads() -> usize {
    rc_data::env_str!("RUSTCRAFT_THREADS")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
}

/// Rewrites `level.dat` (the old one becomes `level.dat_old`), like a save.
pub fn write_level(dir: &Path, root: &Nbt) -> Result<(), Error> {
    let path = dir.join("level.dat");
    crate::save::replace(&path, &nbtio::gzip(&nbtio::encode_root(root)), true).map_err(|e| Error::io(&path, e))
}

/// Applies `f` to the `Data` compound of `level.dat` and rewrites the file.
fn edit_level(dir: &Path, f: impl FnOnce(&mut Nbt)) -> Result<(), Error> {
    let mut root = read_level(dir, false)?;
    let d = nbtio::get_mut(&mut root, "Data").ok_or_else(|| Error::Format("level.dat without Data".into()))?;
    f(d);
    write_level(dir, &root)
}

/// "Edit World" → "Save": new `LevelName` (edge spaces removed).
pub fn rename(dir: &Path, name: &str) -> Result<(), Error> {
    edit_level(dir, |d| set(d, "LevelName", Nbt::String(name.trim().to_owned())))
}

/// Data packs of a new world (`DataPacks`) and enabled features (`enabled_features`,
/// written only beyond `minecraft:vanilla`).
pub fn set_data_config(dir: &Path, enabled: &[String], disabled: &[String], features: &[String]) -> Result<(), Error> {
    let list = |v: &[String]| Nbt::List(v.iter().map(|s| Nbt::String(s.clone())).collect());
    edit_level(dir, |d| {
        set(
            d,
            "DataPacks",
            Nbt::Compound(vec![("Enabled".into(), list(enabled)), ("Disabled".into(), list(disabled))]),
        );
        if features.iter().any(|f| f != "minecraft:vanilla") {
            set(d, "enabled_features", list(features));
        } else {
            nbtio::remove(d, "enabled_features");
        }
    })
}

/// Recovery: `level.dat_old` copied over `level.dat` (readable only).
pub fn restore_old(dir: &Path) -> bool {
    let old = dir.join("level.dat_old");
    if read_level(dir, true).is_err() {
        return false;
    }
    let path = dir.join("level.dat");
    match std::fs::read(&old) {
        Ok(bytes) => crate::save::replace(&path, &bytes, false).is_ok(),
        Err(_) => false,
    }
}

/// Threshold of the low disk space warning when opening a world (64 MiB).
pub const LOW_DISK: u64 = 64 * 1024 * 1024;

/// Free space usable by this process on the folder's volume; unknown (error, or
/// system without a reading): `u64::MAX`, like the game which then does not warn.
#[allow(unsafe_code)]
pub fn usable_space(dir: &Path) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        }
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let mut avail = 0u64;
        // SAFETY: zero-terminated string, optional outputs null, `avail` written by the call.
        let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
        if ok != 0 { avail } else { u64::MAX }
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        u64::MAX
    }
}

/// Less than `LOW_DISK` free under the world: warning before opening it.
pub fn low_disk_space(dir: &Path) -> bool {
    is_low(usable_space(dir))
}

fn is_low(free: u64) -> bool {
    free < LOW_DISK
}

/// Local time zone offset in seconds (daylight saving time included), 0 if unknown.
#[allow(unsafe_code)]
pub fn local_offset_secs() -> i64 {
    #[cfg(windows)]
    {
        #[repr(C)]
        struct SystemTime {
            v: [u16; 8],
        }
        #[repr(C)]
        struct TimeZoneInformation {
            bias: i32,
            standard_name: [u16; 32],
            standard_date: SystemTime,
            standard_bias: i32,
            daylight_name: [u16; 32],
            daylight_date: SystemTime,
            daylight_bias: i32,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetTimeZoneInformation(tz: *mut TimeZoneInformation) -> u32;
        }
        // SAFETY: structure of the size expected by the API, filled in by the call.
        let mut tz: TimeZoneInformation = unsafe { std::mem::zeroed() };
        let r = unsafe { GetTimeZoneInformation(&mut tz) };
        let bias = match r {
            1 => tz.bias + tz.standard_bias,
            2 => tz.bias + tz.daylight_bias,
            0 => tz.bias,
            _ => return 0,
        };
        -i64::from(bias) * 60
    }
    #[cfg(not(windows))]
    {
        0
    }
}

/// Local time zone at instant `ms` (the system's daylight saving rules): offset in seconds,
/// daylight saving, zone key (`Romance Standard Time`); `None` outside Windows or on failure.
#[allow(unsafe_code)]
pub fn local_zone_at(ms: i64) -> Option<(i64, bool, String)> {
    #[cfg(windows)]
    {
        #[repr(C)]
        #[derive(Clone, Copy, Default)]
        struct SystemTime {
            v: [u16; 8],
        }
        #[repr(C)]
        struct DynamicTimeZone {
            bias: i32,
            standard_name: [u16; 32],
            standard_date: SystemTime,
            standard_bias: i32,
            daylight_name: [u16; 32],
            daylight_date: SystemTime,
            daylight_bias: i32,
            key_name: [u16; 128],
            dynamic_disabled: u8,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDynamicTimeZoneInformation(tz: *mut DynamicTimeZone) -> u32;
            fn SystemTimeToTzSpecificLocalTimeEx(
                tz: *const DynamicTimeZone,
                utc: *const SystemTime,
                local: *mut SystemTime,
            ) -> i32;
        }
        // SAFETY: structure of the size expected by the API, filled in by the call.
        let mut tz: DynamicTimeZone = unsafe { std::mem::zeroed() };
        if unsafe { GetDynamicTimeZoneInformation(&mut tz) } == u32::MAX {
            return None;
        }
        let secs = ms.div_euclid(1000);
        let (y, mo, d, h, mi, s) = civil_from_secs(secs);
        let utc = SystemTime { v: [y as u16, mo as u16, 0, d as u16, h as u16, mi as u16, s as u16, 0] };
        let mut local = SystemTime::default();
        // SAFETY: pointers to structures valid for the whole duration of the call.
        if unsafe { SystemTimeToTzSpecificLocalTimeEx(&tz, &utc, &mut local) } == 0 {
            return None;
        }
        let l = local.v;
        let local_secs = days_from_civil(i64::from(l[0]), u32::from(l[1]), u32::from(l[3])) * 86_400
            + i64::from(l[4]) * 3600
            + i64::from(l[5]) * 60
            + i64::from(l[6]);
        let offset = local_secs - secs;
        let dst = offset != -i64::from(tz.bias + tz.standard_bias) * 60;
        let end = tz.key_name.iter().position(|&c| c == 0).unwrap_or(tz.key_name.len());
        Some((offset, dst, String::from_utf16_lossy(&tz.key_name[..end])))
    }
    #[cfg(not(windows))]
    {
        let _ = ms;
        None
    }
}

/// Days since 1 January 1970 of a date of the Gregorian calendar.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Local date and time (year, month, day, hour, minute, second) of a Unix instant in ms.
pub fn local_datetime(ms: i64) -> (i64, u32, u32, u32, u32, u32) {
    civil_from_secs(ms.div_euclid(1000) + local_offset_secs())
}

/// Date and time (year, month, day, hour, minute, second) of `s` seconds since 1970.
pub fn civil_from_secs(s: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = s.div_euclid(86_400);
    let sec = s.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, (sec / 3600) as u32, (sec / 60 % 60) as u32, (sec % 60) as u32)
}

/// Backup copy ("Make a backup"): `backups/<YYYY-MM-DD_HH-MM-SS>_<folder>.zip`,
/// entries `<folder>/…`, without `session.lock`, dated with the local time of the copy like the
/// game. The entries are compressed side by side (`RUSTCRAFT_OPT_PARALLEL_BACKUP_ZIP=0`: one
/// after the other, the old path). Returns the path and the size of the zip.
pub fn backup(world: &Path, backups: &Path) -> Result<(PathBuf, u64), Error> {
    std::fs::create_dir_all(backups).map_err(|e| Error::io(backups, e))?;
    let id = world.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let now = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
    let local = local_datetime(now);
    let (y, mo, d, h, mi, s) = local;
    let stem = format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}_{id}");
    let name = find_available_name(backups, &stem, ".zip");
    let path = backups.join(name);
    let threads = if parallel_backup_on() { worker_threads() } else { 0 };
    write_backup(world, &id, &path, zip_time(local), threads, BACKUP_BYTES_IN_FLIGHT)?;
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    Ok((path, size))
}

/// Backup entries compressed side by side (`RUSTCRAFT_OPT_PARALLEL_BACKUP_ZIP`, on unless `0`).
fn parallel_backup_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_PARALLEL_BACKUP_ZIP") != Some("0")
}

/// Zip date of a local date and time; 1980-01-01 (the zip default) outside the range zip dates hold.
fn zip_time((y, mo, d, h, mi, s): (i64, u32, u32, u32, u32, u32)) -> zip::DateTime {
    let small = |v: u32| u8::try_from(v).unwrap_or(0);
    u16::try_from(y)
        .ok()
        .and_then(|y| zip::DateTime::from_date_and_time(y, small(mo), small(d), small(h), small(mi), small(s)).ok())
        .unwrap_or_default()
}

/// Writes the backup zip of `world` at `path`: one by one (`threads` 0, the old path) or
/// compressed on `threads` threads with at most `budget` bytes waiting; both give the same bytes.
fn write_backup(
    world: &Path,
    id: &str,
    path: &Path,
    when: zip::DateTime,
    threads: usize,
    budget: u64,
) -> Result<(), Error> {
    let file = std::fs::File::create(path).map_err(|e| Error::io(path, e))?;
    let mut zip = ZipWriter::new(std::io::BufWriter::new(file));
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(true)
        .last_modified_time(when);
    let entries = backup_entries(world, id)?;
    if threads == 0 {
        write_entries_one_by_one(&mut zip, &entries, opts, path)?;
    } else {
        write_entries_side_by_side(&mut zip, &entries, opts, threads, budget)?;
    }
    let mut w = zip.finish().map_err(zip_error)?;
    w.flush().map_err(|e| Error::io(path, e))
}

fn zip_error(e: zip::result::ZipError) -> Error {
    Error::Format(e.to_string())
}

/// Files of the copy as (path, entry name `<id>/…`), in the order the copy writes them: the files
/// of a folder by name, then its sub-folders (the last one first); `session.lock` left out.
fn backup_entries(world: &Path, id: &str) -> Result<Vec<(PathBuf, String)>, Error> {
    let mut out = Vec::new();
    let mut stack = vec![world.to_owned()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let ft = e.file_type().map_err(|er| Error::io(&p, er))?;
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            if p.file_name().is_some_and(|n| n == "session.lock") {
                continue;
            }
            let mut entry = id.to_owned();
            for c in p.strip_prefix(world).unwrap_or(&p).components() {
                entry.push('/');
                entry.push_str(&c.as_os_str().to_string_lossy());
            }
            out.push((p, entry));
        }
    }
    Ok(out)
}

/// The old path: each file read whole, then compressed into the zip.
fn write_entries_one_by_one<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    entries: &[(PathBuf, String)],
    opts: SimpleFileOptions,
    zip_path: &Path,
) -> Result<(), Error> {
    let mut buf = Vec::new();
    for (file, name) in entries {
        buf.clear();
        std::fs::File::open(file).and_then(|mut f| f.read_to_end(&mut buf)).map_err(|er| Error::io(file, er))?;
        zip.start_file(name.as_str(), opts).map_err(zip_error)?;
        zip.write_all(&buf).map_err(|er| Error::io(zip_path, er))?;
    }
    Ok(())
}

/// At most this many input bytes are taken by the compressing threads and not yet copied into the
/// zip (the entry the writer waits for always passes, whatever its size): bounds the memory to
/// about twice this (files read whole, their compressed copies waiting).
const BACKUP_BYTES_IN_FLIGHT: u64 = 64 * 1024 * 1024;

/// What the compressing threads hand to the writer.
#[derive(Default)]
struct Handover {
    /// Memory budget: [`BACKUP_BYTES_IN_FLIGHT`] (smaller in the tests).
    budget: u64,
    /// Entry the writer copies next (all the ones before are in the zip).
    next_to_write: usize,
    /// Input bytes of the entries taken by a thread and not yet copied.
    bytes_in_flight: u64,
    /// Entries compressed and waiting, by rank: each one an archive of a single entry.
    ready: BTreeMap<usize, Result<Vec<u8>, Error>>,
    /// The writer gave up after an error: the threads stop.
    stopped: bool,
}

fn lock(m: &Mutex<Handover>) -> std::sync::MutexGuard<'_, Handover> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Deflating is what costs and it splits by file: every file is compressed on its own thread into
/// an archive of a single entry, then copied raw into the zip in the walk order. A raw copy keeps
/// the compressed bytes and headers as they are, so the zip is the one the old path writes.
fn write_entries_side_by_side<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    entries: &[(PathBuf, String)],
    opts: SimpleFileOptions,
    threads: usize,
    budget: u64,
) -> Result<(), Error> {
    let sizes: Vec<u64> = entries.iter().map(|(p, _)| std::fs::metadata(p).map_or(0, |m| m.len())).collect();
    let next = AtomicUsize::new(0);
    let state = Mutex::new(Handover { budget, ..Handover::default() });
    let changed = Condvar::new();
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, entries.len().max(1)) {
            s.spawn(|| compress_entries(entries, &sizes, opts, &next, &state, &changed));
        }
        let copied = copy_entries_in_order(zip, &sizes, &state, &changed);
        if copied.is_err() {
            lock(&state).stopped = true;
            changed.notify_all();
        }
        copied
    })
}

/// A compressing thread: takes the next entry, waits for room in the memory budget, compresses it.
fn compress_entries(
    entries: &[(PathBuf, String)],
    sizes: &[u64],
    opts: SimpleFileOptions,
    next: &AtomicUsize,
    state: &Mutex<Handover>,
    changed: &Condvar,
) {
    loop {
        let rank = next.fetch_add(1, Ordering::Relaxed);
        let Some((file, name)) = entries.get(rank) else { return };
        let mut st = lock(state);
        // The entry the writer waits for always passes: no deadlock, whatever the file sizes.
        while !st.stopped && rank != st.next_to_write && st.bytes_in_flight + sizes[rank] > st.budget {
            st = changed.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
        if st.stopped {
            return;
        }
        st.bytes_in_flight += sizes[rank];
        drop(st);
        let packed = compress_entry(file, name, opts, sizes[rank]);
        lock(state).ready.insert(rank, packed);
        changed.notify_all();
    }
}

/// One file compressed into an archive of a single entry, in memory. The file is read whole and
/// written in one call, like the old path: the deflater's output depends on how its input is cut
/// (a stream read of 8 KiB pieces gave one byte more on a 1 MiB file), and the zip must not change.
fn compress_entry(file: &Path, name: &str, opts: SimpleFileOptions, size: u64) -> Result<Vec<u8>, Error> {
    let content = std::fs::read(file).map_err(|e| Error::io(file, e))?;
    let room = usize::try_from(size).unwrap_or(0).saturating_add(1024);
    let mut one = ZipWriter::new(Cursor::new(Vec::with_capacity(room)));
    one.start_file(name, opts).map_err(zip_error)?;
    one.write_all(&content).map_err(|e| Error::io(file, e))?;
    Ok(one.finish().map_err(zip_error)?.into_inner())
}

/// The writer: copies the compressed entries raw into the zip, in rank order.
fn copy_entries_in_order<W: Write + Seek>(
    zip: &mut ZipWriter<W>,
    sizes: &[u64],
    state: &Mutex<Handover>,
    changed: &Condvar,
) -> Result<(), Error> {
    for (rank, size) in sizes.iter().enumerate() {
        let packed = {
            let mut st = lock(state);
            loop {
                if let Some(p) = st.ready.remove(&rank) {
                    break p;
                }
                st = changed.wait(st).unwrap_or_else(PoisonError::into_inner);
            }
        };
        zip.merge_archive(ZipArchive::new(Cursor::new(packed?)).map_err(zip_error)?).map_err(zip_error)?;
        let mut st = lock(state);
        st.next_to_write = rank + 1;
        st.bytes_in_flight -= size;
        drop(st);
        changed.notify_all();
    }
    Ok(())
}

/// Deletion of the world ("Delete"): lock taken (refused if the world is open elsewhere),
/// files erased, `session.lock` last; up to 5 attempts.
pub fn delete(world: &Path) -> Result<(), Error> {
    let lock = SessionLock::take(world)?;
    // World folder itself a symbolic link: the game walks it as a plain file and
    // only removes the link; the target keeps everything (its `session.lock` included, just released).
    if std::fs::symlink_metadata(world).is_ok_and(|m| m.file_type().is_symlink()) {
        let r = std::fs::remove_file(world).or_else(|_| std::fs::remove_dir(world));
        lock.release();
        return r.map_err(|e| Error::io(world, e));
    }
    let threads = if parallel_delete_on() { worker_threads() } else { 1 };
    let mut last = Ok(());
    for i in 0..5 {
        last = if threads > 1 {
            remove_tree_side_by_side(world, &world.join("session.lock"), threads)
        } else {
            remove_tree(world, Some(&world.join("session.lock")))
        };
        if last.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100 * (i + 1)));
    }
    lock.release();
    last.map_err(|e| Error::io(world, e))?;
    let _ = std::fs::remove_file(world.join("session.lock"));
    std::fs::remove_dir(world).or_else(|_| std::fs::remove_dir_all(world)).map_err(|e| Error::io(world, e))
}

/// Contents of `dir` erased, never following a symbolic link: only the link is removed
/// (like the game), never what it designates outside the world.
fn remove_tree(dir: &Path, keep: Option<&Path>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if keep.is_some_and(|k| k == p) {
            continue;
        }
        let t = e.file_type()?;
        if t.is_symlink() {
            // Link to a folder on Windows: removed like a folder.
            std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p))?;
        } else if t.is_dir() {
            remove_tree(&p, None)?;
            std::fs::remove_dir(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// World files erased side by side (`RUSTCRAFT_OPT_PARALLEL_WORLD_DELETE`, on unless `0`).
fn parallel_delete_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_PARALLEL_WORLD_DELETE") != Some("0")
}

/// Same erasing as [`remove_tree`], but the files (and links) of the whole tree are removed on
/// `threads` threads, then the emptied folders deepest first: each removal waits on the file
/// system, and several can wait at once.
fn remove_tree_side_by_side(dir: &Path, keep: &Path, threads: usize) -> std::io::Result<()> {
    let mut files = Vec::new();
    let mut folders = Vec::new();
    list_tree(dir, keep, &mut files, &mut folders)?;
    let next = AtomicUsize::new(0);
    let remove_some = || -> std::io::Result<()> {
        while let Some((path, link)) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
            if *link {
                // Link to a folder on Windows: removed like a folder.
                std::fs::remove_file(path).or_else(|_| std::fs::remove_dir(path))?;
            } else {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    };
    std::thread::scope(|s| {
        let helpers: Vec<_> = (1..threads.min(files.len())).map(|_| s.spawn(remove_some)).collect();
        let mine = remove_some();
        helpers.into_iter().map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p))).fold(mine, Result::and)
    })?;
    // Listed parents first: the reverse order empties the deepest folders first.
    folders.iter().rev().try_for_each(std::fs::remove_dir)
}

/// Files (with a "link" flag) and folders under `dir`, never entering a symbolic link; `keep` left out.
fn list_tree(
    dir: &Path,
    keep: &Path,
    files: &mut Vec<(PathBuf, bool)>,
    folders: &mut Vec<PathBuf>,
) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let p = e.path();
        if p == keep {
            continue;
        }
        let t = e.file_type()?;
        if t.is_dir() && !t.is_symlink() {
            folders.push(p.clone());
            list_tree(&p, keep, files, folders)?;
        } else {
            files.push((p, t.is_symlink()));
        }
    }
    Ok(())
}

/// Optimisation progress, read by the screen while the thread works. Like the game, the chunk
/// counters and the pass progress restart with each pass; the dimensions' progress does not.
#[derive(Default)]
pub struct Optimize {
    /// 0 counting, 1 upgrading, 2 finished (last dimension worked on).
    pub stage: AtomicU8,
    /// Pass: 0 none yet, otherwise rank + 1 in [`OPTIMIZE_PASSES`].
    pub pass: AtomicU8,
    /// Chunks of the pass.
    pub total: AtomicU32,
    pub converted: AtomicU32,
    pub skipped: AtomicU32,
    /// Progress (`f32` bits) of each dimension (Overworld, Nether, End): its region files done
    /// over all the region files of the pass.
    pub dim_progress: [AtomicU32; 3],
    /// Progress (`f32` bits) of the pass, in region files.
    pub progress: AtomicU32,
    pub cancel: AtomicBool,
}

impl Optimize {
    pub fn dim_progress(&self, k: usize) -> f32 {
        f32::from_bits(self.dim_progress[k].load(Ordering::Relaxed))
    }

    pub fn progress(&self) -> f32 {
        f32::from_bits(self.progress.load(Ordering::Relaxed))
    }
}

/// Passes of the optimisation in the game's order: (folder of the dimension, status key suffix).
pub const OPTIMIZE_PASSES: [(&str, &str); 3] = [("entities", "entities"), ("poi", "poi"), ("region", "chunks")];

/// Optimisation ("Optimize World"), three passes like the game: entity chunks, points of
/// interest, then block chunks, each dimension in turn. A pass counts the region files holding
/// chunks, then reads every chunk again: in the block pass, `erase_cache` removes stored heights
/// and light and rewrites the chunk ("converted") when any was there; otherwise "skipped". Data
/// from an older version would be converted by the game's data fixers, not reproduced here: left
/// as is and counted "skipped". Cancelling stops after the region files being processed.
/// The region files of a dimension are shared among `RUSTCRAFT_THREADS` threads (default: the
/// logical cores), one file per thread at a time; the dimensions stay one after the other, like the
/// game's progress bars. `RUSTCRAFT_OPT_PARALLEL_WORLD_OPTIMIZE=0`: a single thread (the old path).
pub fn optimize(world: &Path, erase_cache: bool, p: &Optimize) {
    let parallel = rc_data::env_str!("RUSTCRAFT_OPT_PARALLEL_WORLD_OPTIMIZE") != Some("0");
    optimize_with(world, erase_cache, p, if parallel { worker_threads() } else { 1 });
}

fn optimize_with(world: &Path, erase_cache: bool, p: &Optimize, threads: usize) {
    let dims = crate::server::DIMENSIONS;
    for (rank, (folder, _)) in OPTIMIZE_PASSES.iter().enumerate() {
        for c in [&p.total, &p.converted, &p.skipped] {
            c.store(0, Ordering::Relaxed);
        }
        p.pass.store(rank as u8 + 1, Ordering::Release);
        let erase = erase_cache && *folder == "region";
        let files: Vec<Vec<(PathBuf, Vec<usize>)>> =
            dims.iter().map(|d| region_files(&crate::world::dimension_dir(world, d).join(folder))).collect();
        for f in files.iter().flatten() {
            p.total.fetch_add(f.1.len() as u32, Ordering::Relaxed);
        }
        let all = files.iter().map(Vec::len).sum::<usize>() as f32;
        let mut start = 0;
        for (k, list) in files.iter().enumerate() {
            if !list.is_empty() {
                p.stage.store(1, Ordering::Release);
                let next = std::sync::atomic::AtomicUsize::new(0);
                // Files finished, under the lock so that the shown progress only grows.
                let done = std::sync::Mutex::new(0usize);
                let work = || {
                    while !p.cancel.load(Ordering::Acquire) {
                        let Some((path, chunks)) = list.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                        optimize_file(path, chunks, erase, p);
                        let mut d = done.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        *d += 1;
                        p.dim_progress[k].store((*d as f32 / all).to_bits(), Ordering::Relaxed);
                        p.progress.store(((start + *d) as f32 / all).to_bits(), Ordering::Relaxed);
                    }
                };
                let n = threads.clamp(1, list.len());
                if n == 1 {
                    work();
                } else {
                    std::thread::scope(|s| {
                        for _ in 0..n {
                            s.spawn(work);
                        }
                    });
                }
                p.stage.store(2, Ordering::Release);
            }
            start += list.len();
        }
    }
}

/// Region files (`r.<x>.<z>.mca`) of `dir` holding chunks, with their chunks in the game's order
/// (a column of the region after the other).
fn region_files(dir: &Path) -> Vec<(PathBuf, Vec<usize>)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let path = e.path();
        if region_coords(&path).is_none() {
            continue;
        }
        let Ok(r) = RegionFile::open(&path, false) else { continue };
        let mut have = [false; 1024];
        for i in r.present() {
            have[i] = true;
        }
        let chunks: Vec<usize> = (0..32).flat_map(|x| (0..32).map(move |z| x + 32 * z)).filter(|&i| have[i]).collect();
        if !chunks.is_empty() {
            out.push((path, chunks));
        }
    }
    out
}

/// One region file of a pass: each chunk counted "converted" (rewritten) or "skipped".
fn optimize_file(path: &Path, chunks: &[usize], erase: bool, p: &Optimize) {
    let Some((rx, rz)) = region_coords(path) else { return };
    // Without erasing nothing can change (same version) or be converted here (older data): every
    // chunk is skipped without reading it again.
    let opened = if erase { RegionFile::open(path, true).ok() } else { None };
    let Some(mut r) = opened else {
        p.skipped.fetch_add(chunks.len() as u32, Ordering::Relaxed);
        return;
    };
    let mut encoder = nbtio::RootEncoder::default();
    for &i in chunks {
        let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
        let converted = match r.read(i, cx, cz) {
            Ok(Some(bytes)) => match nbtio::parse_root(&bytes) {
                Ok(mut root) => {
                    let current =
                        nbtio::int(&root, "DataVersion").is_some_and(|v| v >= i64::from(crate::chunk::DATA_VERSION));
                    // Counted as soon as it is sent to be written, like the game (a failed
                    // write only reaches its log).
                    current && erase_chunk_cache(&mut root) && {
                        let _ = r.write(i, cx, cz, &encoder.encode(&root));
                        true
                    }
                }
                Err(_) => false,
            },
            _ => false,
        };
        let counter = if converted { &p.converted } else { &p.skipped };
        counter.fetch_add(1, Ordering::Relaxed);
    }
    let _ = r.close();
}

/// Region of a file named exactly `r.<x>.<z>.mca` (optional minus sign and digits only).
fn region_coords(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let (x, z) = name.strip_prefix("r.")?.strip_suffix(".mca")?.split_once('.')?;
    let num = |s: &str| {
        let digits = s.strip_prefix('-').unwrap_or(s);
        (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
    };
    Some((num(x)?, num(z)?))
}

/// Recomputable data of a chunk removed: heights, light flag, section light; true if any was there.
fn erase_chunk_cache(root: &mut Nbt) -> bool {
    let take = |v: &mut Nbt, key: &str| {
        let had = v.get(key).is_some();
        nbtio::remove(v, key);
        had
    };
    let mut had = take(root, "Heightmaps");
    had |= take(root, "isLightOn");
    if let Some(Nbt::List(sections)) = nbtio::get_mut(root, "sections") {
        for s in sections {
            had |= take(s, "BlockLight");
            had |= take(s, "SkyLight");
        }
    }
    had
}

/// Visible flat world preset (`flat_level_generator_preset`, tag `visible`).
#[derive(Debug, Clone)]
pub struct FlatPreset {
    pub id: String,
    /// Displayed item (`minecraft:grass_block`).
    pub display: String,
    /// `settings` compound of the flat generator.
    pub settings: Nbt,
}

/// Data of the creation screens read from the jar: biomes, flat presets.
#[derive(Debug, Clone, Default)]
pub struct CreationData {
    pub biomes: Vec<String>,
    pub flat_presets: Vec<FlatPreset>,
}

fn json_nbt(j: &rc_worldgen::json::Json) -> Nbt {
    use rc_worldgen::json::Json;
    match j {
        Json::Null => Nbt::Compound(Vec::new()),
        Json::Bool(b) => Nbt::Byte(i8::from(*b)),
        Json::Num(s) => s.parse::<i32>().map(Nbt::Int).unwrap_or_else(|_| Nbt::Double(s.parse().unwrap_or(0.0))),
        Json::Str(s) => Nbt::String(s.clone()),
        Json::Arr(a) => Nbt::List(a.iter().map(json_nbt).collect()),
        Json::Obj(m) => Nbt::Compound(m.iter().map(|(k, v)| (k.clone(), json_nbt(v))).collect()),
    }
}

/// Biomes and flat presets of the game (jar reading: to be done off the display thread).
pub fn creation_data(data: &rc_data::VersionData) -> Result<CreationData, Error> {
    let jar = crate::registries::client_jar(data).ok_or_else(|| Error::Refused("game jar not found".into()))?;
    let pack = rc_worldgen::data::DataPack::from_jar(&jar).map_err(|e| Error::Format(e.to_string()))?;
    let mut biomes = pack.ids("biome");
    biomes.sort();
    const VISIBLE: [&str; 9] = [
        "classic_flat",
        "tunnelers_dream",
        "water_world",
        "overworld",
        "snowy_kingdom",
        "bottomless_pit",
        "desert",
        "redstone_ready",
        "the_void",
    ];
    let order: Vec<String> = pack
        .json("tags/worldgen/flat_level_generator_preset", "minecraft:visible")
        .ok()
        .and_then(|j| j.get("values").and_then(|v| v.as_array()).map(|a| a.to_vec()))
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_else(|| VISIBLE.iter().map(|s| format!("minecraft:{s}")).collect());
    let flat_presets = order
        .into_iter()
        .filter_map(|id| {
            let j = pack.json("flat_level_generator_preset", &id).ok()?;
            Some(FlatPreset {
                display: j.get("display").and_then(|d| d.as_str()).unwrap_or("minecraft:stone").to_owned(),
                settings: json_nbt(j.get("settings")?),
                id,
            })
        })
        .collect();
    Ok(CreationData { biomes, flat_presets })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Low space: strictly less than 64 MiB, like the game; the current volume is measured.
    #[test]
    fn low_disk_threshold() {
        assert!(is_low(LOW_DISK - 1));
        assert!(!is_low(LOW_DISK));
        assert!(!is_low(u64::MAX), "reading impossible: no warning");
        let here = usable_space(&std::env::temp_dir());
        assert!(here > 0);
        #[cfg(windows)]
        assert_ne!(here, u64::MAX, "Windows reading");
    }

    #[test]
    fn folder_names() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-{}", std::process::id())));
        std::fs::create_dir_all(dir.join("New World")).unwrap();
        std::fs::create_dir_all(dir.join("New World (1)")).unwrap();
        assert_eq!(find_available_name(&dir, "New World", ""), "New World (2)");
        // The counter of the base is never taken up: a second one is added (game quirk).
        assert_eq!(find_available_name(&dir, "New World (1)", ""), "New World (1) (1)");
        assert_eq!(find_available_name(&dir, "a/b.c:d", ""), "a_b_c_d");
        assert_eq!(find_available_name(&dir, "CON", ""), "_CON_");
        // Bare `COM` is reserved too, bare `LPT` is not.
        assert_eq!(find_available_name(&dir, "com", ""), "_com_");
        assert_eq!(find_available_name(&dir, "LPT", ""), "LPT");
        assert_eq!(find_available_name(&dir, "COM0", ""), "COM0");
        assert_eq!(find_available_name(&dir, "x", ".zip"), "x.zip");
    }

    /// Optimisation: entity chunks, points of interest, then block chunks; only block chunks
    /// carrying a cache are rewritten; the counters restart with each pass; the progress of a
    /// dimension is its share of the pass's region files.
    #[test]
    fn optimize_runs_the_three_passes() {
        let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-o-{}", std::process::id())));
        let chunk = |cache: bool, version: i32| {
            let mut f = vec![("DataVersion".to_owned(), Nbt::Int(version))];
            if cache {
                f.push(("Heightmaps".into(), Nbt::Compound(vec![])));
                let s = Nbt::Compound(vec![("SkyLight".into(), Nbt::ByteArray(vec![0; 2048]))]);
                f.push(("sections".into(), Nbt::List(vec![s])));
            }
            nbtio::encode_root(&Nbt::Compound(f))
        };
        let put = |dim: &str, folder: &str, file: &str, chunks: &[(usize, bool, i32)]| {
            let path = crate::world::dimension_dir(&world, dim).join(folder).join(file);
            let mut r = RegionFile::open_or_create(&path).unwrap();
            for &(i, cache, v) in chunks {
                r.write(i, (i % 32) as i32, (i / 32) as i32, &chunk(cache, v)).unwrap();
            }
            r.close().unwrap();
        };
        let now = crate::chunk::DATA_VERSION;
        put("minecraft:overworld", "region", "r.0.0.mca", &[(0, true, now), (1, false, now), (33, true, now - 1)]);
        put("minecraft:overworld", "region", "r.0.0.mca.bak", &[(0, true, now)]);
        put("minecraft:the_nether", "region", "r.0.0.mca", &[(5, true, now)]);
        put("minecraft:overworld", "entities", "r.0.0.mca", &[(0, false, now), (1, false, now)]);
        let p = Optimize::default();
        optimize(&world, true, &p);
        let n = |a: &AtomicU32| a.load(Ordering::Relaxed);
        assert_eq!((p.stage.load(Ordering::Relaxed), p.pass.load(Ordering::Relaxed)), (2, 3));
        // Block pass only: 4 chunks, 2 rewritten (cache, current version), an older one left.
        assert_eq!((n(&p.total), n(&p.converted), n(&p.skipped)), (4, 2, 2));
        assert_eq!((p.dim_progress(0), p.dim_progress(1), p.dim_progress(2), p.progress()), (0.5, 0.5, 0.0, 1.0));
        let mut r = RegionFile::open(
            &crate::world::dimension_dir(&world, "minecraft:overworld").join("region/r.0.0.mca"),
            false,
        )
        .unwrap();
        let read = |r: &mut RegionFile, i: usize| {
            nbtio::parse_root(&r.read(i, (i % 32) as i32, (i / 32) as i32).unwrap().unwrap()).unwrap()
        };
        let c0 = read(&mut r, 0);
        assert!(c0.get("Heightmaps").is_none() && nbtio::list(c0.get("sections"))[0].get("SkyLight").is_none());
        assert!(read(&mut r, 33).get("Heightmaps").is_some(), "older data left as is");
        // Without erasing: nothing rewritten, every chunk skipped.
        let p = Optimize::default();
        optimize(&world, false, &p);
        assert_eq!((n(&p.total), n(&p.converted), n(&p.skipped)), (4, 0, 4));
        // Cancelled before starting: counted, nothing processed.
        let p = Optimize::default();
        p.cancel.store(true, Ordering::Relaxed);
        optimize(&world, true, &p);
        assert_eq!((n(&p.total), n(&p.converted) + n(&p.skipped)), (4, 0));
        assert_eq!(region_coords(Path::new("r.-1.+2.mca")), None);
        assert_eq!(region_coords(Path::new("r.-1.2.mca")), Some((-1, 2)));
        assert_eq!(region_coords(Path::new("r.1.2.3.mca")), None);
    }

    /// Micro-bench: optimisation of 4 region files of 1024 chunks, without then with erasing,
    /// against a plain read of every chunk (what the pass without erasing used to do).
    #[test]
    #[ignore = "micro-bench"]
    fn bench_optimize() {
        let world = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-ob-{}", std::process::id())));
        let dir = crate::world::dimension_dir(&world, "minecraft:overworld").join("region");
        let mut root = vec![("DataVersion".to_owned(), Nbt::Int(crate::chunk::DATA_VERSION))];
        root.push(("Heightmaps".into(), Nbt::Compound(vec![("WORLD_SURFACE".into(), Nbt::LongArray(vec![7; 37]))])));
        let sections = (0..24).map(|_| Nbt::Compound(vec![("SkyLight".into(), Nbt::ByteArray(vec![15; 2048]))]));
        root.push(("sections".into(), Nbt::List(sections.collect())));
        let bytes = nbtio::encode_root(&Nbt::Compound(root));
        const FILES: i32 = 8;
        let fill = || {
            for f in 0..FILES {
                let mut r = RegionFile::open_or_create(&dir.join(format!("r.{f}.0.mca"))).unwrap();
                for i in 0..1024 {
                    r.write(i, f * 32 + (i % 32) as i32, (i / 32) as i32, &bytes).unwrap();
                }
                r.close().unwrap();
            }
        };
        fill();
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        for pass in 0..2 {
            let t = std::time::Instant::now();
            for f in 0..FILES {
                let mut r = RegionFile::open(&dir.join(format!("r.{f}.0.mca")), false).unwrap();
                for i in 0..1024 {
                    let _ = std::hint::black_box(r.read(i, f * 32 + (i % 32) as i32, (i / 32) as i32));
                }
            }
            let read_all = t.elapsed();
            let t = std::time::Instant::now();
            optimize(&world, false, &Optimize::default());
            eprintln!("pass {pass}: read all {read_all:?}, optimise without erasing {:?}", t.elapsed());
            let mut threads = 1;
            while threads <= cores.min(FILES as usize) {
                fill();
                let t = std::time::Instant::now();
                optimize_with(&world, true, &Optimize::default(), threads);
                eprintln!("pass {pass}: optimise + erase, {threads} threads: {:?}", t.elapsed());
                threads *= 2;
            }
        }
    }

    #[test]
    fn backup_and_delete() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-b-{}", std::process::id())));
        let w = dir.join("saves").join("World");
        std::fs::create_dir_all(w.join("data")).unwrap();
        std::fs::write(w.join("level.dat"), b"abc").unwrap();
        std::fs::write(w.join("data").join("x.dat"), b"defg").unwrap();
        std::fs::write(w.join("session.lock"), b"x").unwrap();
        let (zip_path, size) = backup(&w, &dir.join("backups")).unwrap();
        assert!(size > 0);
        assert!(zip_path.file_name().unwrap().to_string_lossy().ends_with("_World.zip"));
        let mut a = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> =
            (0..a.len()).map(|i| a.by_index(i).unwrap().name().unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["World/data/x.dat", "World/level.dat"]);
        delete(&w).unwrap();
        assert!(!w.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A small world of nested folders, files of varied sizes (empty, tiny, several MiB of
    /// incompressible then repetitive bytes) and a `session.lock`; the same each call.
    fn sample_world(world: &Path) {
        let mut noise = 0x2545_f491_4f6c_dd1du64;
        let mut bytes = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|i| {
                    noise ^= noise << 13;
                    noise ^= noise >> 7;
                    noise ^= noise << 17;
                    if i % 3 == 0 { b'a' } else { noise as u8 }
                })
                .collect()
        };
        let files: [(&str, usize); 9] = [
            ("level.dat", 456),
            ("level.dat_old", 0),
            ("session.lock", 3),
            ("data/raids.dat", 90),
            ("dimensions/minecraft/overworld/region/r.0.0.mca", 3 << 20),
            ("dimensions/minecraft/overworld/region/r.-1.0.mca", 700_000),
            ("dimensions/minecraft/overworld/entities/r.0.0.mca", 8192),
            ("dimensions/minecraft/the_nether/region/r.0.0.mca", 1 << 20),
            ("players/data/0000.dat", 2000),
        ];
        for (rel, n) in files {
            let p = world.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, bytes(n)).unwrap();
        }
    }

    /// The backup compressed side by side is the zip of the old path, byte for byte (same entries,
    /// order, compressed data, dates), on 1 to 8 threads and with a memory budget smaller than one
    /// file (the writer's entry always passes); entries dated with the copy's local time.
    #[test]
    fn backup_side_by_side_writes_the_old_zip() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-bp-{}", std::process::id())));
        let world = dir.join("World");
        sample_world(&world);
        let when = zip_time((2026, 11, 3, 14, 5, 36));
        let zip_with = |threads: usize, budget: u64| {
            let path = dir.join(format!("copy-{threads}-{budget}.zip"));
            write_backup(&world, "World", &path, when, threads, budget).unwrap();
            std::fs::read(&path).unwrap()
        };
        let old = zip_with(0, 0);
        for threads in [1, 2, 3, 8] {
            for budget in [BACKUP_BYTES_IN_FLIGHT, 1000] {
                let new = zip_with(threads, budget);
                if new != old {
                    let at = old.iter().zip(&new).position(|(a, b)| a != b).unwrap_or_else(|| old.len().min(new.len()));
                    let lo = at.saturating_sub(16);
                    panic!(
                        "{threads} threads, budget {budget}: other bytes at {at} (len {} / {}): {:02x?} / {:02x?}",
                        old.len(),
                        new.len(),
                        &old[lo..(at + 32).min(old.len())],
                        &new[lo..(at + 32).min(new.len())]
                    );
                }
            }
        }
        let mut a = zip::ZipArchive::new(Cursor::new(&old)).unwrap();
        let names: Vec<String> = a.file_names().map(|n| n.unwrap().into_owned()).collect();
        assert_eq!(names.len(), 8, "{names:?}");
        assert_eq!(names[..3], ["World/level.dat", "World/level.dat_old", "World/players/data/0000.dat"]);
        let mut e = a.by_name("World/dimensions/minecraft/overworld/region/r.0.0.mca").unwrap();
        assert_eq!(
            e.last_modified().map(|t| (t.year(), t.month(), t.day(), t.hour(), t.minute())),
            Some((2026, 11, 3, 14, 5))
        );
        let mut back = Vec::new();
        e.read_to_end(&mut back).unwrap();
        assert_eq!(back.len(), 3 << 20);
        assert_eq!(zip_time((1970, 1, 1, 0, 0, 0)), zip::DateTime::default(), "before 1980");
    }

    /// Deletion side by side: the same tree erased (nested folders, `session.lock` kept for the
    /// end), like the old path.
    #[test]
    fn delete_side_by_side_erases_like_one_by_one() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-dp-{}", std::process::id())));
        for (name, threads) in [("old", 1), ("two", 2), ("many", 8)] {
            let world = dir.join(name);
            sample_world(&world);
            let lock = world.join("session.lock");
            if threads == 1 {
                remove_tree(&world, Some(&lock)).unwrap();
            } else {
                remove_tree_side_by_side(&world, &lock, threads).unwrap();
            }
            let left: Vec<_> = std::fs::read_dir(&world).unwrap().flatten().map(|e| e.file_name()).collect();
            assert_eq!(left, ["session.lock"], "{name}");
        }
    }

    /// Optimisation on one thread or many: the same chunks rewritten (NBT read back identical) and
    /// the same counters and progress.
    #[test]
    fn optimize_on_many_threads_matches_one_thread() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-op-{}", std::process::id())));
        let chunk = |i: usize, cache: bool| {
            let mut f = vec![
                ("DataVersion".to_owned(), Nbt::Int(crate::chunk::DATA_VERSION)),
                ("xPos".into(), Nbt::Int(i as i32)),
            ];
            if cache {
                f.push(("isLightOn".into(), Nbt::Byte(1)));
                let s = Nbt::Compound(vec![("BlockLight".into(), Nbt::ByteArray(vec![i as u8; 2048]))]);
                f.push(("sections".into(), Nbt::List(vec![s])));
            }
            nbtio::encode_root(&Nbt::Compound(f))
        };
        let regions = [("minecraft:overworld", 6), ("minecraft:the_nether", 3), ("minecraft:the_end", 1)];
        let build = |world: &Path| {
            for (dim, files) in regions {
                for f in 0..files {
                    let path = crate::world::dimension_dir(world, dim).join("region").join(format!("r.{f}.0.mca"));
                    let mut r = RegionFile::open_or_create(&path).unwrap();
                    for i in (0..1024).step_by(7) {
                        r.write(i, f * 32 + (i % 32) as i32, (i / 32) as i32, &chunk(i, i % 3 != 0)).unwrap();
                    }
                    r.close().unwrap();
                }
            }
        };
        let read_back = |world: &Path| {
            let mut all = Vec::new();
            for (dim, files) in regions {
                for f in 0..files {
                    let path = crate::world::dimension_dir(world, dim).join("region").join(format!("r.{f}.0.mca"));
                    let mut r = RegionFile::open(&path, false).unwrap();
                    for i in (0..1024).step_by(7) {
                        let bytes = r.read(i, f * 32 + (i % 32) as i32, (i / 32) as i32).unwrap().unwrap();
                        all.push(nbtio::parse_root(&bytes).unwrap());
                    }
                }
            }
            all
        };
        let run = |name: &str, threads: usize| {
            let world = dir.join(name);
            build(&world);
            let p = Optimize::default();
            optimize_with(&world, true, &p, threads);
            let n = |a: &AtomicU32| a.load(Ordering::Relaxed);
            let progress = [p.dim_progress(0), p.dim_progress(1), p.dim_progress(2), p.progress()];
            ((n(&p.total), n(&p.converted), n(&p.skipped)), progress, read_back(&world))
        };
        let one = run("one", 1);
        assert_eq!(one.0, (1470, 980, 490));
        assert!(one.2[1].get("isLightOn").is_none() && one.2[0].get("isLightOn").is_none());
        for threads in [2, 4, 16] {
            assert!(run(&format!("t{threads}"), threads) == one, "{threads} threads");
        }
    }

    /// `level.dat` of an older world whose player holds a string the full read refuses: the
    /// summary read skips the player (like the game), the full read still fails.
    #[test]
    fn level_summary_skips_the_player() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-ls-{}", std::process::id())));
        let player = Nbt::Compound(vec![("CustomName".into(), Nbt::String("MARK".into()))]);
        let data = Nbt::Compound(vec![
            ("Player".into(), player),
            ("LevelName".into(), Nbt::String("Old".into())),
            ("WorldGenSettings".into(), Nbt::Compound(vec![("seed".into(), Nbt::Long(3))])),
        ]);
        let mut raw = nbtio::encode_root(&Nbt::Compound(vec![("Data".into(), data)]));
        let at = raw.windows(4).position(|w| w == b"MARK").unwrap();
        raw[at..at + 4].fill(0xff);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("level.dat"), nbtio::gzip(&raw)).unwrap();
        assert!(read_level(&dir, false).is_err());
        let root = read_level_summary(&dir).unwrap();
        let want = Nbt::Compound(vec![("LevelName".into(), Nbt::String("Old".into()))]);
        assert_eq!(root.get("Data"), Some(&want));
        std::fs::write(dir.join("level.dat"), nbtio::gzip(&raw[..at])).unwrap();
        assert!(read_level_summary(&dir).is_err(), "truncated");
    }

    /// Micro-bench: `level.dat` of an older world (player with a full inventory and a recipe book
    /// of 800 entries) read whole, then as the list reads it (player skipped); 26.3 worlds have no
    /// player there, so both reads are the same size for them. 2 passes, best of 200.
    #[test]
    #[ignore = "micro-bench"]
    fn bench_level_summary_read() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-lb-{}", std::process::id())));
        std::fs::create_dir_all(&dir).unwrap();
        let item = |k: usize| {
            Nbt::Compound(vec![
                ("Slot".into(), Nbt::Byte(k as i8)),
                ("id".into(), Nbt::String(format!("minecraft:item_{k}"))),
                ("count".into(), Nbt::Int(64)),
                (
                    "components".into(),
                    Nbt::Compound(vec![("minecraft:custom_name".into(), Nbt::String("Sword".into()))]),
                ),
            ])
        };
        let recipes = Nbt::List((0..800).map(|k| Nbt::String(format!("minecraft:recipe_number_{k}"))).collect());
        let player = Nbt::Compound(vec![
            ("Inventory".into(), Nbt::List((0..36).map(item).collect())),
            ("recipeBook".into(), Nbt::Compound(vec![("recipes".into(), recipes)])),
        ]);
        let data = Nbt::Compound(vec![
            ("LevelName".into(), Nbt::String("Old".into())),
            ("LastPlayed".into(), Nbt::Long(5)),
            ("Player".into(), player),
        ]);
        let raw = nbtio::gzip(&nbtio::encode_root(&Nbt::Compound(vec![("Data".into(), data)])));
        std::fs::write(dir.join("level.dat"), &raw).unwrap();
        for pass in 0..2 {
            let best = |f: &dyn Fn()| {
                (0..200)
                    .map(|_| {
                        let t = std::time::Instant::now();
                        f();
                        t.elapsed()
                    })
                    .min()
                    .unwrap_or_default()
            };
            let whole = best(&|| drop(std::hint::black_box(read_level(&dir, false).unwrap())));
            let summary = best(&|| drop(std::hint::black_box(read_level_summary(&dir).unwrap())));
            eprintln!("pass {pass}: {} bytes gzip, whole read {whole:?}, summary read {summary:?}", raw.len());
        }
    }

    /// Micro-bench: backup copy of the test world (`RUSTCRAFT_SOLO_WORLD`), old path then side by
    /// side on 1, 2, 4… threads up to the logical cores, 2 passes.
    #[test]
    #[ignore = "micro-bench"]
    fn bench_backup() {
        let Some(world) = rc_data::env_os!("RUSTCRAFT_SOLO_WORLD").map(PathBuf::from) else {
            panic!("RUSTCRAFT_SOLO_WORLD missing: no world to copy");
        };
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-bb-{}", std::process::id())));
        std::fs::create_dir_all(&dir).unwrap();
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let when = zip_time((2026, 11, 3, 12, 0, 0));
        for pass in 0..2 {
            let mut threads = 0;
            loop {
                let path = dir.join(format!("b{pass}-{threads}.zip"));
                let t = std::time::Instant::now();
                write_backup(&world, "world", &path, when, threads, BACKUP_BYTES_IN_FLIGHT).unwrap();
                let size = std::fs::metadata(&path).unwrap().len();
                eprintln!("pass {pass}: threads {threads} (0 = old path): {:?}, {size} bytes", t.elapsed());
                std::fs::remove_file(&path).unwrap();
                if threads >= cores {
                    break;
                }
                threads = if threads == 0 { 1 } else { (threads * 2).min(cores) };
            }
        }
    }

    /// Micro-bench: deletion of a world of 640 files of 64 KiB in 10 folders, old path then side
    /// by side on 2, 4… threads up to the logical cores, 2 passes.
    #[test]
    #[ignore = "micro-bench"]
    fn bench_delete() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-db-{}", std::process::id())));
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let fill = |world: &Path| {
            for f in 0..10 {
                let sub = world.join(format!("region{f}"));
                std::fs::create_dir_all(&sub).unwrap();
                for i in 0..64 {
                    std::fs::write(sub.join(format!("r.{i}.0.mca")), vec![f as u8; 64 << 10]).unwrap();
                }
            }
            std::fs::write(world.join("session.lock"), b"x").unwrap();
        };
        for pass in 0..2 {
            let mut threads = 1;
            loop {
                let world = dir.join(format!("w{pass}-{threads}"));
                fill(&world);
                let lock = world.join("session.lock");
                let t = std::time::Instant::now();
                if threads == 1 {
                    remove_tree(&world, Some(&lock)).unwrap();
                } else {
                    remove_tree_side_by_side(&world, &lock, threads).unwrap();
                }
                eprintln!("pass {pass}: threads {threads} (1 = old path): {:?}", t.elapsed());
                if threads >= cores {
                    break;
                }
                threads = (threads * 2).min(cores);
            }
        }
    }

    /// Deletion of a world containing a link to an outside folder: the link goes, the
    /// target stays intact (the old code followed the link and emptied the target).
    #[test]
    fn delete_never_follows_links() {
        let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-storage-l-{}", std::process::id())));
        let w = dir.join("World");
        let outside = dir.join("elsewhere");
        std::fs::create_dir_all(&w).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(w.join("level.dat"), b"abc").unwrap();
        std::fs::write(outside.join("precious"), b"x").unwrap();
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&outside, w.join("link")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, w.join("link")).is_ok();
        if !made {
            eprintln!("symbolic links not allowed on this system: test ignored");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        delete(&w).unwrap();
        assert!(!w.exists());
        assert!(outside.join("precious").is_file());
        // World folder itself a link to an outside folder: only the link goes.
        let target = dir.join("target");
        std::fs::create_dir_all(target.join("region")).unwrap();
        std::fs::write(target.join("level.dat"), b"abc").unwrap();
        std::fs::write(target.join("region").join("r.0.0.mca"), b"r").unwrap();
        let linked = dir.join("LinkedWorld");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&target, &linked).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &linked).unwrap();
        delete(&linked).unwrap();
        assert!(std::fs::symlink_metadata(&linked).is_err(), "the link must be removed");
        assert!(target.join("level.dat").is_file(), "the target outside the game must not be emptied");
        assert!(target.join("region").join("r.0.0.mca").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
