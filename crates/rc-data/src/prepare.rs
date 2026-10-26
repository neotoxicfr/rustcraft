//! Preparation of a version's data from the player's installation, then cache.
//!
//! 1. Locates `.minecraft`, the client jar and the launcher's Java.
//! 2. Downloads the official server jar (URL and SHA-1 fingerprint of the version file).
//! 3. Runs the official generator (reports) then our extractor (facts).
//! 4. Cross-checks and compiles into `data.bin`, reloaded directly on later launches.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::model::VersionData;
use crate::{Error, Result, bin, compile};

/// Extractor source, embedded in the binary and written into the cache on use.
const EXTRACTOR_SRC: &str = include_str!("../../../tools/extractor/Extractor.java");
const CLIENT_EXTRACTOR_SRC: &str = include_str!("../../../tools/extractor/ClientExtractor.java");
const MODEL_EXTRACTOR_SRC: &str = include_str!("../../../tools/extractor/ModelExtractor.java");
const CREATIVE_EXTRACTOR_SRC: &str = include_str!("../../../tools/extractor/CreativeExtractor.java");
const COMMANDS_EXTRACTOR_SRC: &str = include_str!("../../../tools/extractor/CommandsExtractor.java");

/// Integrated-server command trees by permission level (`CommandsExtractor.java`),
/// written next to `data.bin`: 5 × (u32 big-endian length, body of the `commands` packet).
pub fn commands_path(version: &str) -> PathBuf {
    cache_root().join(version).join("commands.bin")
}

/// Sources included in the fingerprint, in order. `ModelExtractor.java` is not among them (original
/// oversight): adding it would change the fingerprint and force every cache to be rebuilt; to be decided separately.
const TAGGED_SOURCES: [&str; 4] = [EXTRACTOR_SRC, CLIENT_EXTRACTOR_SRC, CREATIVE_EXTRACTOR_SRC, COMMANDS_EXTRACTOR_SRC];

/// 64-bit FNV-1a of the concatenation of the sources, evaluated at compile time.
const fn fnv_sources(srcs: &[&str]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut s = 0;
    while s < srcs.len() {
        let b = srcs[s].as_bytes();
        let mut i = 0;
        while i < b.len() {
            h = (h ^ b[i] as u64).wrapping_mul(0x0100_0000_01b3);
            i += 1;
        }
        s += 1;
    }
    h
}

const EXTRACTOR_TAG: u64 = fnv_sources(&TAGGED_SOURCES);

/// FNV-1a fingerprint of the extractor: a modified extractor invalidates existing caches.
fn extractor_tag() -> u64 {
    EXTRACTOR_TAG
}

/// Single pass over the jar's shared index for the base pack JSONs
/// (`RUSTCRAFT_OPT_PREPSINGLE=0`: one jar opening per folder, old path).
fn prep_single() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_str!("RUSTCRAFT_OPT_PREPSINGLE") != Some("0"))
}

/// Jar reads while the JVMs run (`RUSTCRAFT_OPT_PREPOVERLAP=0`: afterwards, old path).
fn prep_overlap() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_str!("RUSTCRAFT_OPT_PREPOVERLAP") != Some("0"))
}

/// What the preparation reads from the client jar besides the Java programs.
type JarReads = (Vec<crate::DimensionType>, Vec<crate::Biome>, Vec<crate::Timeline>, i32);

fn read_client_jar(jar: &Path) -> Result<JarReads> {
    Ok((read_dimension_types(jar)?, read_biomes(jar)?, read_timelines(jar)?, read_protocol(jar)?))
}

#[derive(Debug, Clone)]
pub struct Install {
    pub mc_dir: PathBuf,
    pub version: String,
    pub version_json: PathBuf,
    pub client_jar: PathBuf,
    pub java: Option<PathBuf>,
}

impl Install {
    /// Asset index of the version (`assets/indexes/<id>.json`) and objects folder, if they
    /// exist (identifier read from the version JSON, field `assetIndex.id`).
    pub fn asset_index(&self) -> Option<(PathBuf, PathBuf)> {
        let v: Value = serde_json::from_slice(&std::fs::read(&self.version_json).ok()?).ok()?;
        let id = v["assetIndex"]["id"].as_str()?;
        let assets = self.mc_dir.join("assets");
        let index = assets.join("indexes").join(format!("{id}.json"));
        index.is_file().then(|| (index, assets.join("objects")))
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    crate::env_flags::read_os(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Root folder of the cache (`RUSTCRAFT_CACHE`, otherwise `%LOCALAPPDATA%\rustcraft\cache`).
pub fn cache_root() -> PathBuf {
    env_path("RUSTCRAFT_CACHE")
        .unwrap_or_else(|| env_path("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("rustcraft").join("cache"))
}

/// Locates the official installation (`RUSTCRAFT_MC_DIR`, otherwise `%APPDATA%\.minecraft`).
pub fn locate(version: &str) -> Result<Install> {
    let mc_dir = env_path("RUSTCRAFT_MC_DIR")
        .or_else(|| env_path("APPDATA").map(|a| a.join(".minecraft")))
        .ok_or_else(|| Error::Missing("game folder not found".into()))?;
    let vdir = mc_dir.join("versions").join(version);
    let version_json = vdir.join(format!("{version}.json"));
    let client_jar = vdir.join(format!("{version}.jar"));
    if !version_json.is_file() {
        return Err(Error::Missing(format!("version {version} not installed ({})", version_json.display())));
    }
    let meta = read_json(&version_json)?;
    let component = meta.pointer("/javaVersion/component").and_then(Value::as_str).unwrap_or("java-runtime-epsilon");
    Ok(Install { java: find_java(&mc_dir, component), mc_dir, version: version.to_owned(), version_json, client_jar })
}

/// Launcher's Java (Microsoft Store or classic version), otherwise `JAVA_HOME`.
fn find_java(mc_dir: &Path, component: &str) -> Option<PathBuf> {
    let (platform, exe) = if cfg!(windows) { ("windows-x64", "java.exe") } else { ("linux", "java") };
    let rel = |root: PathBuf| root.join(component).join(platform).join(component).join("bin").join(exe);
    let mut candidates = Vec::new();
    if let Some(p) = env_path("RUSTCRAFT_JAVA") {
        candidates.push(p);
    }
    if let Some(local) = env_path("LOCALAPPDATA") {
        let store =
            local.join("Packages").join("Microsoft.4297127D64EC6_8wekyb3d8bbwe").join("LocalCache").join("Local");
        candidates.push(rel(store.join("runtime")));
    }
    candidates.push(rel(mc_dir.join("runtime")));
    if let Some(home) = env_path("JAVA_HOME") {
        candidates.push(home.join("bin").join(exe));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn read_json(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    serde_json::from_str(&text).map_err(|e| Error::Format(format!("{} : {e}", path.display())))
}

/// Loads the version's data from the cache, or prepares them. `progress` receives
/// one line per step (preparation: a few seconds, only once per version).
pub fn load_or_prepare(version: &str, mut progress: impl FnMut(&str)) -> Result<VersionData> {
    let dir = cache_root().join(version);
    let bin_path = dir.join("data.bin");
    let tag = extractor_tag();
    match load_from(&bin_path, tag) {
        Some(Ok(d)) => return Ok(d),
        Some(Err(e)) => progress(&format!("cache to rebuild: {e}")),
        None => {}
    }
    let install = locate(version)?;
    let d = prepare(&install, &dir, &mut progress)?;
    let tmp = dir.join("data.bin.tmp");
    std::fs::write(&tmp, bin::encode(&d, tag)).map_err(|e| Error::io(&tmp, e))?;
    std::fs::rename(&tmp, &bin_path).map_err(|e| Error::io(&bin_path, e))?;
    progress("data ready");
    Ok(d)
}

/// `None` if the file does not exist, otherwise the decoding result.
fn load_from(path: &Path, tag: u64) -> Option<Result<VersionData>> {
    std::fs::read(path).ok().map(|bytes| bin::decode(&bytes, tag))
}

/// Cached data valid for this executable, never starting a preparation.
pub fn load_cached(version: &str) -> Option<VersionData> {
    load_from(&cache_root().join(version).join("data.bin"), extractor_tag())?.ok()
}

pub fn prepare(install: &Install, dir: &Path, progress: &mut impl FnMut(&str)) -> Result<VersionData> {
    let java = install.java.as_deref().ok_or_else(|| Error::Missing("launcher's Java not found".into()))?;
    std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    let generated = dir.join("generated");
    let facts = dir.join("facts.json");
    let src = dir.join("Extractor.java");
    std::fs::write(&src, EXTRACTOR_SRC).map_err(|e| Error::io(&src, e))?;

    // Fast path: the installed client jar also contains the generator; its libraries
    // are already present. No download, and both programs run at the same
    // time. 26.3 bench: 4.4 s instead of 18 s (download 9 s + 5 s + 3.6 s).
    let mut dimension_types = Vec::new();
    let mut biomes = Vec::new();
    let mut timelines = Vec::new();
    let client_facts = dir.join("client_facts.json");
    let _ = std::fs::remove_file(&client_facts);
    let entity_models = dir.join("entity_models.json");
    let _ = std::fs::remove_file(&entity_models);
    let entity_animations = dir.join("entity_animations.json");
    let _ = std::fs::remove_file(&entity_animations);
    let creative = dir.join("creative.json");
    let _ = std::fs::remove_file(&creative);
    let protocol = if let Some(cp) = client_classpath(install) {
        progress("official reports and fact extraction (client jar)");
        let cp = std::env::join_paths(&cp).map_err(|e| Error::Format(e.to_string()))?;
        let mut generator = Command::new(java);
        generator.current_dir(dir).arg("-cp").arg(&cp).arg("net.minecraft.data.Main");
        generator.arg("--reports").arg("--output").arg(&generated);
        let mut extractor = Command::new(java);
        extractor.current_dir(dir).arg("-cp").arg(&cp).arg(&src).arg(&facts);
        let client_src = dir.join("ClientExtractor.java");
        std::fs::write(&client_src, CLIENT_EXTRACTOR_SRC).map_err(|e| Error::io(&client_src, e))?;
        let mut client = Command::new(java);
        client.current_dir(dir).arg("-cp").arg(&cp).arg(&client_src).arg(&client_facts);
        let model_src = dir.join("ModelExtractor.java");
        std::fs::write(&model_src, MODEL_EXTRACTOR_SRC).map_err(|e| Error::io(&model_src, e))?;
        let mut models = Command::new(java);
        models.current_dir(dir).arg("-cp").arg(&cp).arg(&model_src).arg(&entity_models).arg(&entity_animations);
        let creative_src = dir.join("CreativeExtractor.java");
        std::fs::write(&creative_src, CREATIVE_EXTRACTOR_SRC).map_err(|e| Error::io(&creative_src, e))?;
        let mut tabs = Command::new(java);
        tabs.current_dir(dir).arg("-cp").arg(&cp).arg(&creative_src).arg(&creative);
        let commands_src = dir.join("CommandsExtractor.java");
        std::fs::write(&commands_src, COMMANDS_EXTRACTOR_SRC).map_err(|e| Error::io(&commands_src, e))?;
        let mut commands = Command::new(java);
        commands.current_dir(dir).arg("-cp").arg(&cp).arg(&commands_src).arg(dir.join("commands.bin"));
        let mut cmds = [&mut generator, &mut extractor, &mut client, &mut models, &mut tabs, &mut commands];
        let t = std::time::Instant::now();
        let log = crate::env_flag!("RUSTCRAFT_PREPLOG");
        let (java, reads) = if prep_overlap() {
            std::thread::scope(|s| {
                let reads = s.spawn(|| {
                    let r = read_client_jar(&install.client_jar);
                    if log {
                        eprintln!("preparation: jar reads finished at {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
                    }
                    r
                });
                let java = run_java_together(&mut cmds, log);
                (java, reads.join().unwrap_or_else(|_| Err(Error::Format("jar read interrupted".into()))))
            })
        } else {
            let java = run_java_together(&mut cmds, log);
            let reads = if java.is_ok() { read_client_jar(&install.client_jar) } else { Ok(Default::default()) };
            (java, reads)
        };
        java?;
        let (dims, bios, tls, protocol) = reads?;
        if log {
            eprintln!("preparation: Java and jar finished at {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        }
        (dimension_types, biomes, timelines) = (dims, bios, tls);
        protocol
    } else {
        // Fallback: incomplete game, we go through the downloaded official server jar.
        let server_dir = dir.join("server");
        std::fs::create_dir_all(&server_dir).map_err(|e| Error::io(&server_dir, e))?;
        let server_jar = server_dir.join("server.jar");
        ensure_server_jar(install, &server_jar, progress)?;
        progress("official generator reports (server jar)");
        run_java(
            Command::new(java)
                .current_dir(&server_dir)
                .arg("-DbundlerMainClass=net.minecraft.data.Main")
                .arg("-jar")
                .arg(&server_jar)
                .arg("--reports")
                .arg("--output")
                .arg(&generated),
        )?;
        progress("extraction of block facts");
        let mut cp =
            vec![server_dir.join("versions").join(&install.version).join(format!("server-{}.jar", install.version))];
        collect_jars(&server_dir.join("libraries"), &mut cp)?;
        let cp = std::env::join_paths(&cp).map_err(|e| Error::Format(e.to_string()))?;
        run_java(Command::new(java).current_dir(dir).arg("-cp").arg(cp).arg(&src).arg(&facts))?;
        read_protocol(&server_jar)?
    };

    progress("compiling the data");
    let mut d = compile::compile(&facts, &generated.join("reports"), protocol)?;
    d.dimension_types = dimension_types;
    d.biomes = biomes;
    d.timelines = timelines;
    if client_facts.is_file() {
        d.client_states = compile::client_states(&client_facts, d.states.len())?;
    }
    if entity_models.is_file() {
        d.entity_models = compile::entity_models(&entity_models)?;
    }
    if entity_animations.is_file() {
        d.entity_animations = compile::entity_animations(&entity_animations)?;
    }
    if creative.is_file() {
        d.creative = compile::creative(&creative)?;
    }
    if d.version != install.version {
        return Err(Error::Format(format!("extraction for {} instead of {}", d.version, install.version)));
    }
    Ok(d)
}

/// Classpath of the installed game: client jar and libraries of `<version>.json` retained by
/// their rules for the current system. `None` if a file is missing (incomplete game).
fn client_classpath(install: &Install) -> Option<Vec<PathBuf>> {
    if !install.client_jar.is_file() {
        return None;
    }
    let meta = read_json(&install.version_json).ok()?;
    let mut cp = vec![install.client_jar.clone()];
    for lib in meta.get("libraries")?.as_array()? {
        if !rules_allow(lib.get("rules")) {
            continue;
        }
        let Some(path) = lib.pointer("/downloads/artifact/path").and_then(Value::as_str) else { continue };
        let p = install.mc_dir.join("libraries").join(path);
        if !p.is_file() {
            return None;
        }
        cp.push(p);
    }
    Some(cp)
}

/// Rules of a library: refused by default if there are any; the last rule that
/// applies (system absent or the current one, no feature condition) decides.
fn rules_allow(rules: Option<&Value>) -> bool {
    rules_allow_on(rules, launcher_os())
}

/// System name in the launcher's rules (`windows`, `linux`, `osx`).
fn launcher_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "osx",
        os => os,
    }
}

fn rules_allow_on(rules: Option<&Value>, os: &str) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else { return true };
    let mut allowed = false;
    for r in rules {
        let os_ok = r.pointer("/os/name").and_then(Value::as_str).is_none_or(|n| n == os);
        if os_ok && r.get("features").is_none() {
            allowed = r.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}

/// `log` (`RUSTCRAFT_PREPLOG`): duration of each program on the error output (one wait per thread).
fn run_java_together(cmds: &mut [&mut Command], log: bool) -> Result<()> {
    use std::process::Stdio;
    let t = std::time::Instant::now();
    let mut children = Vec::new();
    for c in cmds.iter_mut() {
        let what = c.get_args().last().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
        children.push((
            what,
            c.stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| Error::Java(format!("launch: {e}")))?,
        ));
    }
    let outputs: Vec<_> = if log {
        std::thread::scope(|s| {
            let waits: Vec<_> = children
                .into_iter()
                .map(|(what, c)| {
                    s.spawn(move || {
                        let out = c.wait_with_output();
                        eprintln!("preparation: {what} finished at {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
                        out
                    })
                })
                .collect();
            waits.into_iter().map(|w| w.join().unwrap_or_else(|_| Err(std::io::Error::other("wait")))).collect()
        })
    } else {
        children.into_iter().map(|(_, c)| c.wait_with_output()).collect()
    };
    for out in outputs {
        let out = out.map_err(|e| Error::Java(e.to_string()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let tail: Vec<&str> = err.lines().rev().take(8).collect();
            return Err(Error::Java(format!(
                "code {:?} : {}",
                out.status.code(),
                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
            )));
        }
    }
    Ok(())
}

fn ensure_server_jar(install: &Install, path: &Path, progress: &mut impl FnMut(&str)) -> Result<()> {
    let meta = read_json(&install.version_json)?;
    let url = meta.pointer("/downloads/server/url").and_then(Value::as_str);
    let sha1 = meta.pointer("/downloads/server/sha1").and_then(Value::as_str);
    let (Some(url), Some(sha1)) = (url, sha1) else {
        return Err(Error::Missing("no server jar for this version".into()));
    };
    if std::fs::read(path).is_ok_and(|b| hex_sha1(&b) == sha1) {
        return Ok(());
    }
    progress("downloading the official server jar");
    let mut resp = ureq::get(url).call().map_err(|e| Error::Download(format!("{url} : {e}")))?;
    let mut bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(512 << 20)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::Download(format!("{url} : {e}")))?;
    let got = hex_sha1(&bytes);
    if got != sha1 {
        return Err(Error::Download(format!("SHA-1 fingerprint {got}, expected {sha1}")));
    }
    std::fs::write(path, &bytes).map_err(|e| Error::io(path, e))
}

fn hex_sha1(bytes: &[u8]) -> String {
    Sha1::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn run_java(cmd: &mut Command) -> Result<()> {
    let out = cmd.output().map_err(|e| Error::Java(format!("launch failed: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let tail: Vec<&str> = err.lines().rev().take(8).collect();
    Err(Error::Java(format!(
        "code {:?} : {}",
        out.status.code(),
        tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
    )))
}

fn collect_jars(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
        let p = e.map_err(|e| Error::io(dir, e))?.path();
        if p.is_dir() {
            collect_jars(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "jar") {
            out.push(p);
        }
    }
    Ok(())
}

/// JSON files of the base pack `data/<ns>/<folder>/<name>.json` of the jar: (`ns:name`, contents).
fn data_jsons(jar: &Path, folder: &str) -> Result<Vec<(String, Value)>> {
    data_jsons_with(jar, folder, prep_single() && crate::jar::enabled())
}

/// `ns:name` name of an entry retained by the historical filter: `data/`, then FIRST "/folder/"
/// (hence the `minecraft/tags:…` names of tags stored under `tags/<folder>/`, kept as they are),
/// then a `.json` file with no subfolder.
fn data_json_name(entry: &str, marker: &str) -> Option<String> {
    let (ns, file) = entry.strip_prefix("data/").and_then(|r| r.split_once(marker))?;
    let name = file.strip_suffix(".json").filter(|n| !n.contains('/'))?;
    Some(format!("{ns}:{name}"))
}

#[doc(hidden)]
pub fn data_jsons_with(jar: &Path, folder: &str, single: bool) -> Result<Vec<(String, Value)>> {
    let marker = format!("/{folder}/");
    if single {
        // Jar index shared with the integrated server: decompressed once for the whole process.
        let idx = crate::jar::index(jar).map_err(|e| Error::Format(format!("{} : {e}", jar.display())))?;
        if let Some((entry, e)) = idx.failed().iter().find(|(n, _)| data_json_name(n, &marker).is_some()) {
            return Err(Error::Format(format!("{entry} : {e}")));
        }
        let mut out = Vec::new();
        for (entry, blob) in idx.with_prefix("data/") {
            let Some(name) = data_json_name(entry, &marker) else { continue };
            let text = blob.text().ok_or_else(|| Error::Format(format!("{name}: invalid UTF-8 text")))?;
            let v = serde_json::from_str(&text).map_err(|e| Error::Format(format!("{name} : {e}")))?;
            out.push((name, v));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        return Ok(out);
    }
    let f = std::fs::File::open(jar).map_err(|e| Error::io(jar, e))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Format(format!("{} : {e}", jar.display())))?;
    let mut out = Vec::new();
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| Error::Format(e.to_string()))?;
        let Ok(entry) = e.name().map(|n| n.into_owned()) else { continue };
        let Some(name) = data_json_name(&entry, &marker) else { continue };
        let mut text = String::new();
        e.read_to_string(&mut text).map_err(|e| Error::io(jar, e))?;
        let v = serde_json::from_str(&text).map_err(|e| Error::Format(format!("{name} : {e}")))?;
        out.push((name, v));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Dimension types of the base pack, timelines resolved by the tags of `tags/timeline`.
fn read_dimension_types(jar: &Path) -> Result<Vec<crate::DimensionType>> {
    let tags: hashbrown::HashMap<String, Value> = data_jsons(jar, "tags/timeline")?.into_iter().collect();
    data_jsons(jar, "dimension_type")?
        .into_iter()
        .map(|(name, v)| match (v["min_y"].as_i64(), v["height"].as_i64()) {
            (Some(min_y), Some(height)) => {
                let nether_light = v["cardinal_light"].as_str() == Some("nether");
                let mut timelines = Vec::new();
                match &v["timelines"] {
                    Value::String(s) => resolve_tag(s, &tags, &mut timelines, 0),
                    Value::Array(a) => {
                        for e in a.iter().filter_map(Value::as_str) {
                            resolve_tag(e, &tags, &mut timelines, 0);
                        }
                    }
                    _ => {}
                }
                Ok(crate::DimensionType {
                    name,
                    min_y: min_y as i32,
                    height: height as i32,
                    nether_light,
                    attributes: env_entries(&v["attributes"]),
                    timelines,
                    skybox: match v["skybox"].as_str() {
                        Some("none") => 0,
                        Some("end") => 2,
                        _ => 1,
                    },
                    has_skylight: v["has_skylight"].as_bool().unwrap_or(true),
                    has_ceiling: v["has_ceiling"].as_bool().unwrap_or(false),
                    default_clock: v["default_clock"].as_str().unwrap_or_default().to_owned(),
                    ambient_light: v["ambient_light"].as_f64().unwrap_or(0.0) as f32,
                    audio: audio_attrs(&v["attributes"]),
                })
            }
            _ => Err(Error::Format(format!("{name}: min_y or height absent"))),
        })
        .collect()
}

/// Tag element (`#ns:tag` expanded recursively, otherwise identifier), without duplicates.
fn resolve_tag(e: &str, tags: &hashbrown::HashMap<String, Value>, out: &mut Vec<String>, depth: u32) {
    let full = |s: &str| if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") };
    if let Some(t) = e.strip_prefix('#') {
        if depth > 8 {
            return;
        }
        let values = tags.get(&full(t)).and_then(|v| v["values"].as_array()).cloned().unwrap_or_default();
        for v in &values {
            let id = v.as_str().or_else(|| v["id"].as_str()).unwrap_or_default();
            resolve_tag(id, tags, out, depth + 1);
        }
    } else {
        let id = full(e);
        if !out.contains(&id) {
            out.push(id);
        }
    }
}

/// Retained environment attribute (visuals, sky light level; fire that goes out faster
/// for the integrated server).
fn env_attr(key: &str) -> Option<&str> {
    let k = key.strip_prefix("minecraft:").unwrap_or(key);
    (k.starts_with("visual/")
        || k == "gameplay/sky_light_level"
        || k == "gameplay/increased_fire_burnout"
        || k == "audio/music_volume"
        || k == "audio/firefly_bush_sounds")
        .then_some(k)
}

/// Sound event identifier: string, or object `{ "sound_id": …, "range": … }`.
fn sound_id(v: &Value) -> Option<String> {
    v.as_str().or_else(|| v["sound_id"].as_str()).map(str::to_owned)
}

/// Value of a discrete attribute: raw, or argument of an `override` modifier.
fn discrete<'a>(attrs: &'a Value, key: &str) -> Option<&'a Value> {
    let v = attrs.get(key)?;
    match v.get("modifier").and_then(Value::as_str) {
        Some("override") => v.get("argument"),
        Some(_) => None,
        None => Some(v),
    }
}

/// Discrete audio attributes (`audio/background_music`, `audio/ambient_sounds`) of a layer.
fn audio_attrs(attrs: &Value) -> crate::AudioAttrs {
    let track = |v: &Value| -> Option<crate::MusicTrack> {
        Some(crate::MusicTrack {
            sound: sound_id(&v["sound"])?,
            min_delay: v["min_delay"].as_i64().unwrap_or(0) as i32,
            max_delay: v["max_delay"].as_i64().unwrap_or(0) as i32,
            replace: v["replace_current_music"].as_bool().unwrap_or(false),
        })
    };
    let music = discrete(attrs, "minecraft:audio/background_music").map(|m| crate::BackgroundMusic {
        default: m.get("default").and_then(track),
        creative: m.get("creative").and_then(track),
        underwater: m.get("underwater").and_then(track),
    });
    let ambient = discrete(attrs, "minecraft:audio/ambient_sounds").map(|a| {
        let addition = |v: &Value| Some((sound_id(&v["sound"])?, v["tick_chance"].as_f64()?));
        let additions = match &a["additions"] {
            Value::Array(l) => l.iter().filter_map(addition).collect(),
            Value::Object(_) => addition(&a["additions"]).into_iter().collect(),
            _ => Vec::new(),
        };
        let m = &a["mood"];
        crate::AmbientSounds {
            loop_sound: a.get("loop").and_then(sound_id),
            mood: sound_id(&m["sound"]).map(|sound| crate::AmbientMood {
                sound,
                tick_delay: m["tick_delay"].as_i64().unwrap_or(6000) as i32,
                block_search_extent: m["block_search_extent"].as_i64().unwrap_or(8) as i32,
                offset: m["offset"].as_f64().unwrap_or(2.0),
            }),
            additions,
        }
    });
    crate::AudioAttrs { ambient, music }
}

/// JSON colour as (r, g, b, a): `#RRGGBB`, `#AARRGGBB`, ARGB integer or list of floats.
fn env_color(v: &Value) -> Option<[f32; 4]> {
    let argb = |c: u32, alpha: bool| {
        let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
        [ch(16), ch(8), ch(0), if alpha { ch(24) } else { 1.0 }]
    };
    match v {
        Value::String(s) => {
            let h = s.trim_start_matches('#');
            u32::from_str_radix(h, 16).ok().map(|c| argb(c, h.len() == 8))
        }
        Value::Number(n) => n.as_i64().map(|c| argb(c as u32, true)),
        Value::Array(a) => {
            let f: Vec<f32> = a.iter().filter_map(Value::as_f64).map(|x| x as f32).collect();
            (f.len() >= 3).then(|| [f[0], f[1], f[2], f.get(3).copied().unwrap_or(1.0)])
        }
        _ => None,
    }
}

/// Modifier argument according to the attribute type and the operation.
fn env_arg(attr: &str, op: crate::EnvOp, v: &Value) -> Option<[f32; 4]> {
    let num = |v: &Value| v.as_f64().map(|x| x as f32);
    match op {
        crate::EnvOp::BlendToGray => Some([num(&v["brightness"])?, num(&v["factor"])?, 0.0, 0.0]),
        crate::EnvOp::AlphaBlend if !crate::env_is_color(attr) => match v {
            Value::Object(_) => Some([num(&v["value"])?, v.get("alpha").and_then(num).unwrap_or(1.0), 0.0, 0.0]),
            _ => Some([num(v)?, 1.0, 0.0, 0.0]),
        },
        _ if crate::env_is_color(attr) => env_color(v),
        _ => match v {
            Value::Bool(b) => Some([f32::from(u8::from(*b)), 0.0, 0.0, 0.0]),
            // Moon phase: index in the game's order.
            Value::String(s) if attr == "visual/moon_phase" => {
                let phases = [
                    "full_moon",
                    "waning_gibbous",
                    "third_quarter",
                    "waning_crescent",
                    "new_moon",
                    "waxing_crescent",
                    "first_quarter",
                    "waxing_gibbous",
                ];
                Some([phases.iter().position(|p| p == s)? as f32, 0.0, 0.0, 0.0])
            }
            _ => Some([num(v)?, 0.0, 0.0, 0.0]),
        },
    }
}

/// Attribute entries of an `attributes` object (raw value = `override`).
fn env_entries(attrs: &Value) -> Vec<crate::EnvEntry> {
    let Some(map) = attrs.as_object() else { return Vec::new() };
    let mut out = Vec::new();
    for (key, v) in map {
        let Some(attr) = env_attr(key) else { continue };
        let (op, arg) = match v.get("modifier").and_then(Value::as_str) {
            Some(m) => match crate::EnvOp::parse(m) {
                Some(op) => (op, &v["argument"]),
                None => continue,
            },
            None => (crate::EnvOp::Override, v),
        };
        if let Some(arg) = env_arg(attr, op, arg) {
            out.push(crate::EnvEntry { attr: attr.to_owned(), op, arg });
        }
    }
    out
}

/// Timelines of the base pack: tracks of the retained attributes only.
fn read_timelines(jar: &Path) -> Result<Vec<crate::Timeline>> {
    Ok(data_jsons(jar, "timeline")?
        .into_iter()
        .map(|(name, v)| {
            let mut tracks = Vec::new();
            if let Some(map) = v["tracks"].as_object() {
                for (key, t) in map {
                    let Some(attr) = env_attr(key) else { continue };
                    let op = t["modifier"].as_str().and_then(crate::EnvOp::parse).unwrap_or(crate::EnvOp::Override);
                    let ease = match &t["ease"] {
                        Value::String(s) if s == "constant" => crate::Ease::Constant,
                        Value::Object(o) => match o.get("cubic_bezier").and_then(Value::as_array) {
                            Some(c) if c.len() == 4 => {
                                let f = |i: usize| c[i].as_f64().unwrap_or(0.0) as f32;
                                crate::Ease::Bezier([f(0), f(1), f(2), f(3)])
                            }
                            _ => crate::Ease::Linear,
                        },
                        _ => crate::Ease::Linear,
                    };
                    let keys = t["keyframes"]
                        .as_array()
                        .map(|k| {
                            k.iter()
                                .filter_map(|k| Some((k["ticks"].as_u64()? as u32, env_arg(attr, op, &k["value"])?)))
                                .collect()
                        })
                        .unwrap_or_default();
                    tracks.push(crate::Track { attr: attr.to_owned(), op, ease, keys });
                }
            }
            let clock = v["clock"].as_str().unwrap_or("minecraft:overworld").to_owned();
            crate::Timeline { name, clock, period: v["period_ticks"].as_u64().unwrap_or(0) as u32, tracks }
        })
        .collect())
}

fn hex_color(v: &Value) -> Option<u32> {
    match v {
        Value::String(s) => u32::from_str_radix(s.trim_start_matches('#'), 16).ok(),
        Value::Number(n) => n.as_u64().map(|n| n as u32 & 0xff_ffff),
        _ => None,
    }
}

/// Biomes of the base pack.
fn read_biomes(jar: &Path) -> Result<Vec<crate::Biome>> {
    Ok(data_jsons(jar, "worldgen/biome")?
        .into_iter()
        .map(|(name, v)| {
            let fx = &v["effects"];
            crate::Biome {
                name,
                temperature: v["temperature"].as_f64().unwrap_or(0.5) as f32,
                downfall: v["downfall"].as_f64().unwrap_or(0.5) as f32,
                grass_color: fx.get("grass_color").and_then(hex_color),
                foliage_color: fx.get("foliage_color").and_then(hex_color),
                dry_foliage_color: fx.get("dry_foliage_color").and_then(hex_color),
                water_color: fx.get("water_color").and_then(hex_color).unwrap_or(0x3f76e4),
                grass_modifier: match fx.get("grass_color_modifier").and_then(Value::as_str) {
                    Some("dark_forest") => 1,
                    Some("swamp") => 2,
                    _ => 0,
                },
                sky_color: v["attributes"].get("minecraft:visual/sky_color").and_then(hex_color),
                attributes: env_entries(&v["attributes"]),
                has_precipitation: v["has_precipitation"].as_bool().unwrap_or(true),
                frozen: v["temperature_modifier"].as_str() == Some("frozen"),
                audio: audio_attrs(&v["attributes"]),
            }
        })
        .collect())
}

/// `version.json` embedded in a jar, parsed.
fn read_version_json(jar: &Path) -> Result<Value> {
    let f = std::fs::File::open(jar).map_err(|e| Error::io(jar, e))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Format(format!("{} : {e}", jar.display())))?;
    let mut entry = z.by_name("version.json").map_err(|e| Error::Format(format!("{} : {e}", jar.display())))?;
    let mut text = String::new();
    entry.read_to_string(&mut text).map_err(|e| Error::io(jar, e))?;
    serde_json::from_str(&text).map_err(|e| Error::Format(e.to_string()))
}

/// Protocol number read from the `version.json` embedded in a jar.
fn read_protocol(jar: &Path) -> Result<i32> {
    read_version_json(jar)?
        .get("protocol_version")
        .and_then(Value::as_i64)
        .map(|p| p as i32)
        .ok_or_else(|| Error::Format("protocol_version absent".into()))
}

/// Game identity read from the jar's `version.json` (command `/version`).
#[derive(Debug, Clone, PartialEq)]
pub struct BuildInfo {
    pub id: String,
    pub name: String,
    pub world_version: i32,
    pub series: String,
    pub protocol: i32,
    /// Format of resource packs (major, minor).
    pub resource_pack: (i64, i64),
    /// Format of data packs (major, minor).
    pub data_pack: (i64, i64),
    /// Build date as written (ISO 8601 with offset).
    pub build_time: String,
    pub stable: bool,
}

/// `BuildInfo` of the jar; required field absent: error (like the game, which refuses a corrupt
/// file), `main` series by default.
pub fn build_info(jar: &Path) -> Result<BuildInfo> {
    parse_build_info(&read_version_json(jar)?)
}

fn parse_build_info(v: &Value) -> Result<BuildInfo> {
    let miss = |k: &str| Error::Format(format!("version.json : {k} absent"));
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| miss(k));
    let i = |o: &Value, k: &str| o.get(k).and_then(Value::as_i64).ok_or_else(|| miss(k));
    let pack = v.get("pack_version").ok_or_else(|| miss("pack_version"))?;
    Ok(BuildInfo {
        id: s("id")?,
        name: s("name")?,
        world_version: i(v, "world_version")? as i32,
        series: v.get("series_id").and_then(Value::as_str).unwrap_or("main").to_owned(),
        protocol: i(v, "protocol_version")? as i32,
        resource_pack: (i(pack, "resource_major")?, i(pack, "resource_minor")?),
        data_pack: (i(pack, "data_major")?, i(pack, "data_minor")?),
        build_time: s("build_time")?,
        stable: v.get("stable").and_then(Value::as_bool).ok_or_else(|| miss("stable"))?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Native libraries: each system retains only its own (otherwise, on Linux, Windows-specific
    /// files are missing and the game looks incomplete).
    #[test]
    fn library_rules_follow_the_system() {
        let only = |os: &str| serde_json::json!([{ "action": "allow", "os": { "name": os } }]);
        let win = only("windows");
        let lin = only("linux");
        assert!(rules_allow_on(Some(&win), "windows") && !rules_allow_on(Some(&win), "linux"));
        assert!(rules_allow_on(Some(&lin), "linux") && !rules_allow_on(Some(&lin), "windows"));
        let all_but_osx = serde_json::json!([{ "action": "allow" }, { "action": "disallow", "os": { "name": "osx" } }]);
        assert!(rules_allow_on(Some(&all_but_osx), "linux") && !rules_allow_on(Some(&all_but_osx), "osx"));
        assert!(rules_allow_on(None, "linux"));
        assert_eq!(launcher_os(), if cfg!(windows) { "windows" } else { std::env::consts::OS });
    }

    #[test]
    fn fingerprint_constant_matches_computation() {
        let runtime = EXTRACTOR_SRC
            .bytes()
            .chain(CLIENT_EXTRACTOR_SRC.bytes())
            .chain(CREATIVE_EXTRACTOR_SRC.bytes())
            .chain(COMMANDS_EXTRACTOR_SRC.bytes())
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
        assert_eq!(extractor_tag(), runtime);
        assert_eq!(fnv_sources(&["ab", "c"]), fnv_sources(&["a", "bc"]));
    }

    /// `/version` identity read from the 26.3 jar: values of the file (fails if the jar is missing).
    #[test]
    fn jar_identity_26_3() {
        let jar = crate::locate("26.3").map(|i| i.client_jar).expect("26.3 installation not found");
        let b = build_info(&jar).expect("version.json");
        assert_eq!((b.id.as_str(), b.name.as_str(), b.world_version, b.series.as_str()), ("26.3", "26.3", 5023, "main"));
        assert_eq!((b.protocol, b.resource_pack, b.data_pack, b.stable), (777, (97, 1), (121, 0), true));
        assert_eq!(b.build_time, "2026-09-15T11:20:48+00:00");
        // Series absent: `main`; required field absent: error.
        let v: Value = serde_json::from_str(
            r#"{"id":"x","name":"x","world_version":1,"protocol_version":2,"build_time":"t","stable":false,
               "pack_version":{"resource_major":1,"resource_minor":2,"data_major":3,"data_minor":4}}"#,
        )
        .unwrap();
        assert_eq!(parse_build_info(&v).unwrap().series, "main");
        let mut bad = v.clone();
        bad.as_object_mut().unwrap().remove("stable");
        assert!(parse_build_info(&bad).is_err());
    }

    /// The four folders read by the preparation: single pass (jar index) == one opening
    /// per folder, down to the JSON, stored fingerprint. Explicit failure if the 26.3 jar is missing.
    #[test]
    fn single_pass_matches_the_old_one() {
        let jar = crate::locate("26.3").map(|i| i.client_jar).expect("26.3 installation not found");
        assert!(jar.is_file(), "jar client 26.3 absent");
        let mut h = Sha1::new();
        for folder in ["tags/timeline", "dimension_type", "timeline", "worldgen/biome"] {
            let old = data_jsons_with(&jar, folder, false).unwrap();
            let new = data_jsons_with(&jar, folder, true).unwrap();
            assert!(!old.is_empty(), "{folder} empty");
            assert!(old == new, "{folder}: content differs");
            for (n, v) in &new {
                h.update(n.as_bytes());
                h.update(v.to_string().as_bytes());
            }
        }
        // Phantom entries of the historical filter, kept bit for bit.
        let biomes = data_jsons_with(&jar, "worldgen/biome", true).unwrap();
        assert!(biomes.iter().any(|(n, _)| n.starts_with("minecraft/tags:")));
        let got: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(got, EXPECTED_JSONS, "fingerprint of the base pack JSONs");
    }

    const EXPECTED_JSONS: &str = "46669f559a6c0d16e0656d09d5549fe0c641a4b5";
}
