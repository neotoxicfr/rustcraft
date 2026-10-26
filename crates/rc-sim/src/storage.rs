//! World folders as seen by the "Singleplayer" screens (`anvil-world.md`): free folder name,
//! rewriting of `level.dat` (name, data packs), zip backup copy in `backups`,
//! restoration of `level.dat_old`, deletion and optimisation (chunk rewriting).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use rc_proto::nbt::Nbt;

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

/// Reserved Windows name (`CON`, `COM1`…, or ending with a dot): refused as is.
fn reserved(name: &str) -> bool {
    if name.ends_with('.') {
        return true;
    }
    let up = name.to_ascii_uppercase();
    let stem = up.split('.').next().unwrap_or("");
    matches!(stem, "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
}

/// First free name in `dir` for `base` (+ `suffix`): cleaned name, "Name (n)" taken up and
/// counted, suffix "(n)" added as long as the folder exists (case ignored on Windows).
pub fn find_available_name(dir: &Path, base: &str, suffix: &str) -> String {
    let mut base = sanitize_name(base);
    if reserved(&base) {
        base = format!("_{base}_");
    }
    let mut count = 0u32;
    if let Some(open) = base.rfind(" (")
        && base.ends_with(')')
    {
        let digits = &base[open + 2..base.len() - 1];
        if digits.chars().all(|c| c.is_ascii_digit()) {
            count = digits.parse().unwrap_or(0);
            base.truncate(open);
        }
    }
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
/// entries `<folder>/…`, without `session.lock`. Returns the path and the size of the zip.
pub fn backup(world: &Path, backups: &Path) -> Result<(PathBuf, u64), Error> {
    std::fs::create_dir_all(backups).map_err(|e| Error::io(backups, e))?;
    let id = world.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let now = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
    let (y, mo, d, h, mi, s) = local_datetime(now);
    let stem = format!("{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{s:02}_{id}");
    let name = find_available_name(backups, &stem, ".zip");
    let path = backups.join(name);
    let file = std::fs::File::create(&path).map_err(|e| Error::io(&path, e))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let mut stack = vec![world.to_owned()];
    let mut buf = Vec::new();
    while let Some(dir) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
        let mut entries: Vec<_> = rd.flatten().collect();
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
            let rel = p.strip_prefix(world).unwrap_or(&p);
            let mut entry = id.clone();
            for c in rel.components() {
                entry.push('/');
                entry.push_str(&c.as_os_str().to_string_lossy());
            }
            buf.clear();
            std::fs::File::open(&p).and_then(|mut f| f.read_to_end(&mut buf)).map_err(|er| Error::io(&p, er))?;
            zip.start_file(entry, opts).map_err(|er| Error::Format(er.to_string()))?;
            zip.write_all(&buf).map_err(|er| Error::io(&path, er))?;
        }
    }
    let mut w = zip.finish().map_err(|er| Error::Format(er.to_string()))?;
    w.flush().map_err(|e| Error::io(&path, e))?;
    drop(w);
    let size = std::fs::metadata(&path).map_or(0, |m| m.len());
    Ok((path, size))
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
    let mut last = Ok(());
    for i in 0..5 {
        last = remove_tree(world, Some(&world.join("session.lock")));
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

/// Optimisation progress, read by the screen while the thread works.
#[derive(Default)]
pub struct Optimize {
    /// 0 counting, 1 rewriting, 2 finished, 3 cancelled, 4 failed.
    pub stage: AtomicU8,
    pub total: AtomicU32,
    pub converted: AtomicU32,
    pub skipped: AtomicU32,
    /// Chunks processed per dimension (Overworld, Nether, End).
    pub done: [AtomicU32; 3],
    pub per_dim: [AtomicU32; 3],
    pub cancel: AtomicBool,
}

/// Optimisation ("Optimize World"): each chunk of the three dimensions is read again; data from
/// an older version cannot be converted here (counted "ignored");
/// `erase_cache` removes stored heights and light then rewrites the chunk ("converted").
pub fn optimize(world: &Path, erase_cache: bool, p: &Optimize) {
    let dims = crate::server::DIMENSIONS;
    let mut work: Vec<(usize, PathBuf)> = Vec::new();
    for (k, d) in dims.iter().enumerate() {
        let dir = crate::world::dimension_dir(world, d).join("region");
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().is_some_and(|x| x == "mca") {
                let n = RegionFile::open(&path, false).map_or(0, |r| r.present().count() as u32);
                p.total.fetch_add(n, Ordering::Relaxed);
                p.per_dim[k].fetch_add(n, Ordering::Relaxed);
                work.push((k, path));
            }
        }
    }
    p.stage.store(1, Ordering::Release);
    for (k, path) in work {
        let Some((rx, rz)) = region_coords(&path) else { continue };
        let Ok(mut r) = RegionFile::open(&path, erase_cache) else { continue };
        let present: Vec<usize> = r.present().collect();
        for i in present {
            if p.cancel.load(Ordering::Acquire) {
                p.stage.store(3, Ordering::Release);
                return;
            }
            let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
            let rewritten = match r.read(i, cx, cz) {
                Ok(Some(bytes)) if erase_cache => match nbtio::parse_root(&bytes) {
                    Ok(mut root) => {
                        erase_chunk_cache(&mut root);
                        r.write(i, cx, cz, &nbtio::encode_root(&root)).is_ok()
                    }
                    Err(_) => false,
                },
                _ => false,
            };
            let counter = if rewritten { &p.converted } else { &p.skipped };
            counter.fetch_add(1, Ordering::Relaxed);
            p.done[k].fetch_add(1, Ordering::Relaxed);
        }
        if erase_cache && r.close().is_err() {
            p.stage.store(4, Ordering::Release);
            return;
        }
    }
    p.stage.store(2, Ordering::Release);
}

fn region_coords(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let mut it = name.strip_prefix("r.")?.strip_suffix(".mca")?.split('.');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// Recomputable data of a chunk: heights, light flag, section light.
fn erase_chunk_cache(root: &mut Nbt) {
    nbtio::remove(root, "Heightmaps");
    nbtio::remove(root, "isLightOn");
    if let Some(Nbt::List(sections)) = nbtio::get_mut(root, "sections") {
        for s in sections {
            nbtio::remove(s, "BlockLight");
            nbtio::remove(s, "SkyLight");
        }
    }
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
        let dir = std::env::temp_dir().join(format!("rc-storage-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("New World")).unwrap();
        std::fs::create_dir_all(dir.join("New World (1)")).unwrap();
        assert_eq!(find_available_name(&dir, "New World", ""), "New World (2)");
        assert_eq!(find_available_name(&dir, "New World (1)", ""), "New World (2)");
        assert_eq!(find_available_name(&dir, "a/b.c:d", ""), "a_b_c_d");
        assert_eq!(find_available_name(&dir, "CON", ""), "_CON_");
        assert_eq!(find_available_name(&dir, "x", ".zip"), "x.zip");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_and_delete() {
        let dir = std::env::temp_dir().join(format!("rc-storage-b-{}", std::process::id()));
        let w = dir.join("saves").join("World");
        std::fs::create_dir_all(w.join("data")).unwrap();
        std::fs::write(w.join("level.dat"), b"abc").unwrap();
        std::fs::write(w.join("data").join("x.dat"), b"defg").unwrap();
        std::fs::write(w.join("session.lock"), b"x").unwrap();
        let (zip_path, size) = backup(&w, &dir.join("backups")).unwrap();
        assert!(size > 0);
        assert!(zip_path.file_name().unwrap().to_string_lossy().ends_with("_World.zip"));
        let mut a = zip::ZipArchive::new(std::fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..a.len()).map(|i| a.by_index(i).unwrap().name().unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["World/data/x.dat", "World/level.dat"]);
        delete(&w).unwrap();
        assert!(!w.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Deletion of a world containing a link to an outside folder: the link goes, the
    /// target stays intact (the old code followed the link and emptied the target).
    #[test]
    fn delete_never_follows_links() {
        let dir = std::env::temp_dir().join(format!("rc-storage-l-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
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
