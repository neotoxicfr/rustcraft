// Direct read allowed (test: settings read at launch, outside the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Access to the reference values written by the Java probe (outside the repository) and to the game data.
#![allow(dead_code)]

use std::path::PathBuf;

/// Folder of the references (`RUSTCRAFT_WORLDGEN_REF`, otherwise `%LOCALAPPDATA%\rustcraft-re\26.3\worldgen-ref`).
/// `None` (test skipped) if it does not exist; failure under `RUSTCRAFT_OPT_REQUIRE=1`.
pub fn ref_dir() -> Option<PathBuf> {
    let p = std::env::var_os("RUSTCRAFT_WORLDGEN_REF").map(PathBuf::from).or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(|l| PathBuf::from(l).join("rustcraft-re").join("26.3").join("worldgen-ref"))
    });
    rc_testdata::need(p.filter(|p| p.is_dir()), "worldgen probe references (RUSTCRAFT_WORLDGEN_REF)")
}

/// Installed client jar 26.3 (`RUSTCRAFT_MC_DIR` or `%APPDATA%\.minecraft`); `None` (test skipped)
/// if missing, failure under `RUSTCRAFT_OPT_REQUIRE=1`.
pub fn client_jar() -> Option<PathBuf> {
    let jar = rc_data::locate("26.3").ok().map(|i| i.client_jar);
    rc_testdata::need(jar.filter(|j| j.is_file()), "jar client 26.3")
}

pub fn read_lines(name: &str) -> Option<Vec<String>> {
    let p = ref_dir()?.join(name);
    Some(
        std::fs::read_to_string(&p)
            .unwrap_or_else(|e| panic!("{} : {e}", p.display()))
            .lines()
            .map(str::to_owned)
            .collect(),
    )
}

pub fn read_f32_bin(name: &str) -> Option<Vec<f32>> {
    let p = ref_dir()?.join(name);
    let b = std::fs::read(&p).unwrap_or_else(|e| panic!("{} : {e}", p.display()));
    Some(b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
}

pub fn hex_f32(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap_or_else(|e| panic!("{s} : {e}")))
}

/// Game data: installed client jar (`RUSTCRAFT_MC_DIR` or `%APPDATA%\.minecraft`) and official
/// reports (`%LOCALAPPDATA%\rustcraft-re\26.3\reports\reports`, or the rc-data cache, `RUSTCRAFT_CACHE`).
pub fn data_pack() -> Option<rc_worldgen::data::DataPack> {
    use std::sync::OnceLock;
    static PACK: OnceLock<Option<rc_worldgen::data::DataPack>> = OnceLock::new();
    PACK.get_or_init(|| {
        let jar = client_jar()?;
        let mut pack = rc_worldgen::data::DataPack::from_jar(&jar).expect("jar read");
        let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default());
        let candidates = [
            local.join("rustcraft-re").join("26.3").join("reports").join("reports"),
            rc_data::cache_root().join("26.3").join("generated").join("reports"),
        ];
        if let Some(r) = candidates.iter().find(|p| p.join("biome_parameters").is_dir()) {
            pack.add_reports(r).expect("reports");
        }
        Some(pack)
    })
    .clone()
}
