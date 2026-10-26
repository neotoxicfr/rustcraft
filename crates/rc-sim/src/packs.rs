//! Repository of a world's data packs, like game 26.3: base pack (`vanilla`),
//! experimental feature packs stored in the jar (`data/minecraft/datapacks/<id>`) and
//! packs from the world's `datapacks` folder (folder with a `pack.mcmeta`, or `.zip` archive).
//! The selection starts from `DataPacks` and `enabled_features` of level.dat; the chosen packs are
//! then stacked in order on the jar's data: a file replaces the one below it,
//! a tag is added to it (except `replace`). Everything that reads the jar afterwards (registries and
//! tags, recipes and loot, functions, advancements) therefore sees the stack.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_proto::nbt::Nbt;
use serde_json::Value;

use crate::registries::Jar;

pub const VANILLA: &str = "vanilla";
pub const VANILLA_FLAG: &str = "minecraft:vanilla";
/// Known feature flags, in their registry order.
pub const FLAGS: [&str; 4] =
    ["minecraft:vanilla", "minecraft:trade_rebalance", "minecraft:redstone_experiments", "minecraft:minecart_improvements"];
/// Folder of the feature packs in the jar.
pub(crate) const BUILTIN_DIR: &str = "data/minecraft/datapacks/";
/// Current format (major, minor) of the data packs.
const FORMAT: (i64, i64) = (crate::progress::DATA_PACK_FORMAT as i64, 0);
/// "All" minor of an upper bound written without a minor.
const ANY_MINOR: i64 = i32::MAX as i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    BuiltIn,
    Feature,
    World,
}

impl Source {
    /// Translation key of the source (`/datapack list`).
    pub fn key(self) -> &'static str {
        match self {
            Source::BuiltIn => "pack.source.builtin",
            Source::Feature => "pack.source.feature",
            Source::World => "pack.source.world",
        }
    }

    /// Is a new pack from this source added automatically? (Not the feature packs.)
    fn automatic(self) -> bool {
        self != Source::Feature
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Loc {
    Vanilla,
    /// Prefix in the jar (`data/minecraft/datapacks/<id>/`).
    Jar(String),
    Dir(PathBuf),
    Zip(PathBuf),
}

/// Available pack.
#[derive(Debug, Clone, PartialEq)]
pub struct Pack {
    pub id: String,
    pub source: Source,
    /// Description text (JSON of the `pack.mcmeta`).
    pub description: Value,
    /// Requested flags, in registry order.
    pub features: Vec<String>,
    /// Overlay folders applicable to the current format, in declared order.
    overlays: Vec<String>,
    loc: Loc,
    /// `filter` section of the `pack.mcmeta` (`{"filter": …}`), compiled at stacking: it hides
    /// the files of the packs placed below.
    filter: Option<Value>,
}

impl Pack {
    /// Title (`/datapack list`): that of the base pack, otherwise its name.
    pub fn title(&self) -> rc_text::Component {
        match self.source {
            Source::BuiltIn => rc_text::Component::translate("dataPack.vanilla.name", Vec::new()),
            Source::Feature => rc_text::Component::text(self.id.clone()),
            Source::World => rc_text::Component::text(self.id.strip_prefix("file/").unwrap_or(&self.id).to_owned()),
        }
    }

    /// Description as text (fallback: empty).
    pub fn description_text(&self) -> rc_text::Component {
        rc_text::from_nbt(&crate::recipes::json_to_nbt(&self.description)).unwrap_or_else(|| rc_text::Component::text(""))
    }
}

/// Built-in packs: the base pack and the jar's feature packs.
pub(crate) fn builtin(jar: &Jar) -> Vec<Pack> {
    let mut out = vec![Pack {
        id: VANILLA.into(),
        source: Source::BuiltIn,
        description: serde_json::json!({ "translate": "dataPack.vanilla.description" }),
        features: vec![VANILLA_FLAG.into()],
        overlays: Vec::new(),
        filter: None,
        loc: Loc::Vanilla,
    }];
    for (name, bytes) in &jar.files {
        let Some(id) = name.strip_prefix(BUILTIN_DIR).and_then(|r| r.strip_suffix("/pack.mcmeta")) else { continue };
        if id.contains('/') {
            continue;
        }
        if let Some(m) = parse_meta(bytes) {
            out.push(m.into_pack(id.to_owned(), Source::Feature, Loc::Jar(format!("{BUILTIN_DIR}{id}/"))));
        }
    }
    out
}

/// `builtin` packs and packs from the world's `datapacks` folder (reread on every call,
/// like the game before every list or reload), sorted by id.
pub(crate) fn with_world(builtin: &[Pack], world: &Path) -> Vec<Pack> {
    let mut out = builtin.to_vec();
    if let Ok(rd) = std::fs::read_dir(world.join("datapacks")) {
        for e in rd.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            // Symbolic links refused: the game only follows those of its allowed list,
            // empty by default.
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let (bytes, loc) = if meta.is_dir() {
                (std::fs::read(path.join("pack.mcmeta")).ok(), Loc::Dir(path))
            } else if meta.is_file() && name.ends_with(".zip") {
                (zip_entry(&path, "pack.mcmeta"), Loc::Zip(path))
            } else {
                continue;
            };
            match bytes.as_deref().and_then(parse_meta) {
                Some(m) => out.push(m.into_pack(format!("file/{name}"), Source::World, loc)),
                None => eprintln!("singleplayer: data pack {name} ignored (pack.mcmeta missing or invalid)"),
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

struct Meta {
    description: Value,
    features: Vec<String>,
    overlays: Vec<String>,
    filter: Option<Value>,
}

impl Meta {
    fn into_pack(self, id: String, source: Source, loc: Loc) -> Pack {
        Pack {
            id,
            source,
            description: self.description,
            features: self.features,
            overlays: self.overlays,
            loc,
            filter: self.filter,
        }
    }
}

/// `pack.mcmeta`: mandatory description (otherwise the pack is ignored, like the game, whose fallback
/// read only keeps it); unknown flags or invalid overlays section:
/// pack ignored.
fn parse_meta(bytes: &[u8]) -> Option<Meta> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let description = v.get("pack")?.get("description")?;
    if description.is_null() {
        return None;
    }
    let mut features = Vec::new();
    if let Some(f) = v.get("features") {
        for x in f.get("enabled")?.as_array()? {
            let id = id_of(x.as_str()?);
            if !FLAGS.contains(&id.as_str()) {
                return None;
            }
            features.push(id);
        }
    }
    let features = FLAGS.iter().filter(|f| features.iter().any(|x| x == *f)).map(|f| (*f).to_owned()).collect();
    let mut overlays = Vec::new();
    if let Some(o) = v.get("overlays") {
        for e in o.get("entries")?.as_array()? {
            let dir = e.get("directory")?.as_str()?;
            if dir.is_empty() || !dir.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_-/".contains(&c)) {
                return None;
            }
            match overlay_range(e) {
                Err(()) => return None,
                Ok(None) => eprintln!("singleplayer: overlay {dir} without formats, ignored"),
                Ok(Some((lo, hi))) => {
                    if lo <= FORMAT && FORMAT <= hi {
                        overlays.push(dir.to_owned());
                    }
                }
            }
        }
    }
    // Unreadable `filter` section: the game keeps the pack, without a filter (`stack`).
    let filter = v.get("filter").map(|f| serde_json::json!({ "filter": f }));
    Some(Meta { description: description.clone(), features, overlays, filter })
}

/// Pack format (major, minor).
type Format = (i64, i64);

/// Format written `n` or `[major]` / `[major, minor]` (default minor `minor`).
fn format_of(v: &Value, minor: i64) -> Option<(i64, i64)> {
    let ok = |x: &Value| x.as_i64().filter(|n| (0..=ANY_MINOR).contains(n));
    match v {
        Value::Array(a) if (1..=2).contains(&a.len()) => Some((ok(&a[0])?, a.get(1).map_or(Some(minor), ok)?)),
        Value::Array(_) => None,
        x => Some((ok(x)?, minor)),
    }
}

/// Format range of an overlay: `min_format`/`max_format` (both), otherwise `formats`
/// (integer, `[a, b]` or `{min_inclusive, max_inclusive}`, in majors). Neither: entry
/// ignored; inconsistent: error.
fn overlay_range(e: &Value) -> Result<Option<(Format, Format)>, ()> {
    let min = e.get("min_format").map(|x| format_of(x, 0).ok_or(())).transpose()?;
    let max = e.get("max_format").map(|x| format_of(x, ANY_MINOR).ok_or(())).transpose()?;
    let formats = match e.get("formats") {
        None => None,
        Some(Value::Array(a)) if a.len() == 2 => Some((a[0].as_i64().ok_or(())?, a[1].as_i64().ok_or(())?)),
        Some(Value::Object(o)) => Some((
            o.get("min_inclusive").and_then(Value::as_i64).ok_or(())?,
            o.get("max_inclusive").and_then(Value::as_i64).ok_or(())?,
        )),
        Some(x) => {
            let n = x.as_i64().ok_or(())?;
            Some((n, n))
        }
    };
    match (min, max) {
        (Some(lo), Some(hi)) => {
            if lo > hi {
                return Err(());
            }
            Ok(Some((lo, hi)))
        }
        (Some(_), None) | (None, Some(_)) => Err(()),
        (None, None) => match formats {
            Some((a, b)) if a <= b => Ok(Some(((a, 0), (b, ANY_MINOR)))),
            Some(_) => Err(()),
            None => Ok(None),
        },
    }
}

/// Id completed with the `minecraft` namespace.
fn id_of(s: &str) -> String {
    if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") }
}

/// Computed selection: chosen packs in order (the last wins), available ones not
/// chosen, world flags (registry order).
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    pub features: Vec<String>,
}

impl Selection {
    /// Nothing but the base pack: the stack is the jar itself.
    pub fn vanilla_only(&self) -> bool {
        self.enabled.len() == 1 && self.enabled[0] == VANILLA
    }
}

/// Choice of the packs of an existing world, like the game on opening and reloading:
/// the `enabled` ones present in order, then each available one absent from `disabled` added
/// automatically (automatic source and all flags allowed) or removed (missing flags);
/// none: `vanilla`. The world's flags (`world_flags`) are both allowed and imposed:
/// an imposed flag that no chosen pack provides makes the feature pack that carries it be added.
pub fn select(avail: &[Pack], enabled: &[String], disabled: &[String], world_flags: &[String]) -> Selection {
    let subset = |f: &[String]| f.iter().all(|x| world_flags.contains(x));
    let mut sel: Vec<String> = Vec::new();
    for id in enabled {
        if avail.iter().any(|p| p.id == *id) {
            if !sel.contains(id) {
                sel.push(id.clone());
            }
        } else {
            eprintln!("singleplayer: data pack {id} missing");
        }
    }
    for p in avail {
        if disabled.contains(&p.id) {
            continue;
        }
        let chosen = sel.contains(&p.id);
        if !chosen && p.source.automatic() {
            if subset(&p.features) {
                sel.push(p.id.clone());
            } else {
                eprintln!("singleplayer: pack {} not loaded, features missing", p.id);
            }
        }
        if chosen && !subset(&p.features) {
            eprintln!("singleplayer: pack {} disabled, features not enabled for this world", p.id);
            sel.retain(|x| *x != p.id);
        }
    }
    if sel.is_empty() {
        sel.push(VANILLA.into());
    }
    let feats = |sel: &[String]| -> Vec<String> {
        let mut f: Vec<String> = Vec::new();
        for id in sel {
            if let Some(p) = avail.iter().find(|p| p.id == *id) {
                f.extend(p.features.iter().cloned());
            }
        }
        f
    };
    // Imposed flags missing: feature packs that carry them (and nothing else).
    let mut missing: Vec<String> = world_flags.iter().filter(|f| !feats(&sel).contains(f)).cloned().collect();
    for p in avail {
        if missing.is_empty() {
            break;
        }
        if p.source == Source::Feature
            && !p.features.is_empty()
            && p.features.iter().any(|f| missing.contains(f))
            && subset(&p.features)
            && !sel.contains(&p.id)
        {
            sel.push(p.id.clone());
            missing.retain(|f| !p.features.contains(f));
        }
    }
    let mut all = feats(&sel);
    all.extend(world_flags.iter().cloned());
    let features = FLAGS.iter().filter(|f| all.iter().any(|x| x == *f)).map(|f| (*f).to_owned()).collect();
    let disabled = avail.iter().filter(|p| !sel.contains(&p.id)).map(|p| p.id.clone()).collect();
    Selection { enabled: sel, disabled, features }
}

/// Settings read in `Data` of level.dat: (Enabled, Disabled, flags). Absent or
/// unreadable: game defaults (`vanilla` alone, none disabled, `minecraft:vanilla`).
pub fn config_of(data: Option<&Nbt>) -> (Vec<String>, Vec<String>, Vec<String>) {
    let strings = |v: Option<&Nbt>| -> Option<Vec<String>> {
        match v? {
            Nbt::List(l) => l.iter().map(|x| x.as_str().map(str::to_owned)).collect(),
            _ => None,
        }
    };
    let dp = data.and_then(|d| d.get("DataPacks"));
    let (enabled, disabled) = match (strings(dp.and_then(|d| d.get("Enabled"))), strings(dp.and_then(|d| d.get("Disabled")))) {
        (Some(e), Some(d)) => (e, d),
        _ => (vec![VANILLA.to_owned()], Vec::new()),
    };
    let flags = match strings(data.and_then(|d| d.get("enabled_features"))) {
        Some(f) if f.iter().all(|x| FLAGS.contains(&id_of(x).as_str())) => f.iter().map(|x| id_of(x)).collect(),
        _ => vec![VANILLA_FLAG.to_owned()],
    };
    (enabled, disabled, flags)
}

/// Selection written in `Data` of level.dat; each field equal to its default value is omitted.
pub fn write_config(data: &mut Nbt, sel: &Selection) {
    let list = |v: &[String]| Nbt::List(v.iter().map(|s| Nbt::String(s.clone())).collect());
    if sel.vanilla_only() && sel.disabled.is_empty() {
        crate::nbtio::remove(data, "DataPacks");
    } else {
        crate::nbtio::set(
            data,
            "DataPacks",
            Nbt::Compound(vec![("Enabled".into(), list(&sel.enabled)), ("Disabled".into(), list(&sel.disabled))]),
        );
    }
    if sel.features.len() == 1 && sel.features[0] == VANILLA_FLAG {
        crate::nbtio::remove(data, "enabled_features");
    } else {
        crate::nbtio::set(data, "enabled_features", list(&sel.features));
    }
}

/// Full opening of a world: built-in packs, selection, stack placed on `jar`.
pub(crate) fn open(jar: &mut Jar, world: &Path, data: Option<&Nbt>) -> (Vec<Pack>, Selection) {
    let builtin = builtin(jar);
    let avail = with_world(&builtin, world);
    let (enabled, disabled, flags) = config_of(data);
    let sel = select(&avail, &enabled, &disabled, &flags);
    stack(jar, &avail, &sel);
    (builtin, sel)
}

/// Imposed selection (`/datapack enable|disable`, `/reload`): the available `enabled` ones in
/// order, all the other available ones disabled, world flags unchanged.
pub fn explicit(avail: &[Pack], enabled: &[String], features: &[String]) -> Selection {
    let mut sel: Vec<String> = Vec::new();
    for id in enabled {
        if avail.iter().any(|p| p.id == *id) && !sel.contains(id) {
            sel.push(id.clone());
        }
    }
    let disabled = avail.iter().filter(|p| !sel.contains(&p.id)).map(|p| p.id.clone()).collect();
    Selection { enabled: sel, disabled, features: features.to_vec() }
}

/// Stacks the chosen packs, in order, in place of the jar's files. Only the JSON and `.mcfunction`
/// files `data/<namespace>/<path>` with a valid id remain.
pub(crate) fn stack(jar: &mut Jar, avail: &[Pack], sel: &Selection) {
    if sel.vanilla_only() {
        return;
    }
    let all = std::mem::take(&mut jar.files);
    let mut base = BTreeMap::new();
    let mut builtin = BTreeMap::new();
    for (k, v) in all {
        if k.starts_with(BUILTIN_DIR) {
            builtin.insert(k, v);
        } else {
            base.insert(k, v);
        }
    }
    let mut base = Some(base);
    let mut out: BTreeMap<String, Arc<[u8]>> = BTreeMap::new();
    for id in &sel.enabled {
        let Some(p) = avail.iter().find(|p| p.id == *id) else { continue };
        let files: Vec<(String, Arc<[u8]>)> = match &p.loc {
            Loc::Vanilla => {
                let Some(b) = base.take() else { continue };
                // First of the stack (common case): the jar's files are taken as they are.
                if out.is_empty() {
                    out = b;
                    continue;
                }
                b.into_iter().collect()
            }
            Loc::Jar(prefix) => layered(p, builtin.range(prefix.clone()..).take_while(|(k, _)| k.starts_with(prefix.as_str())).map(|(k, v)| (k[prefix.len()..].to_owned(), v.clone()))),
            Loc::Dir(dir) => layered(p, dir_files(dir)),
            Loc::Zip(z) => layered(p, zip_files(z)),
        };
        // `filter` section: the files already placed by the packs below that it
        // designates disappear (tags included), not those of the pack itself.
        if let Some(f) = p.filter.as_ref().and_then(rc_data::pack_filter::filter) {
            out.retain(|k, _| !f.hides_file(k));
        }
        for (k, v) in files {
            put(&mut out, k, v);
        }
    }
    jar.files = out;
}

/// Files of a pack (paths relative to its root) with its overlays applied: the
/// `data/` folder then each overlay in order, the last one winning.
fn layered(p: &Pack, files: impl Iterator<Item = (String, Arc<[u8]>)>) -> Vec<(String, Arc<[u8]>)> {
    let mut by_layer: Vec<BTreeMap<String, Arc<[u8]>>> = vec![BTreeMap::new(); p.overlays.len() + 1];
    for (rel, bytes) in files {
        if rel.starts_with("data/") {
            by_layer[0].insert(rel, bytes);
            continue;
        }
        for (i, o) in p.overlays.iter().enumerate() {
            if let Some(r) = rel.strip_prefix(o.as_str()).and_then(|r| r.strip_prefix('/'))
                && r.starts_with("data/")
            {
                by_layer[i + 1].insert(r.to_owned(), bytes);
                break;
            }
        }
    }
    let mut merged = BTreeMap::new();
    for l in by_layer {
        merged.extend(l);
    }
    merged.into_iter().filter(|(k, _)| keep(k)).collect()
}

/// File kept in the stack: `data/<namespace>/<path>.json|.mcfunction`, valid id.
fn keep(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("data/") else { return false };
    let Some((ns, path)) = rest.split_once('/') else { return false };
    let ns_ok = !ns.is_empty() && ns.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c));
    let path_ok = path.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-/".contains(&c));
    ns_ok && path_ok && (path.ends_with(".json") || path.ends_with(".mcfunction"))
}

/// Content of a valid tag file: `values` list of valid entries, `replace`
/// boolean if written. Otherwise `None` (file rejected).
fn tag_values(v: &Value) -> Option<&Vec<Value>> {
    if v.get("replace").is_some_and(|r| !r.is_boolean()) {
        return None;
    }
    v.get("values").and_then(Value::as_array).filter(|a| a.iter().all(|x| entry_of(x).is_some()))
}

fn is_tag(key: &str) -> bool {
    key.strip_prefix("data/").and_then(|r| r.split_once('/')).is_some_and(|(_, p)| p.starts_with("tags/")) && key.ends_with(".json")
}

/// Places a file on the stack: a tag is added to the one below (or replaces it
/// with `replace`), any other file replaces it. Unreadable JSON: file ignored. JSON read but
/// invalid content: file ignored, the tag exists nonetheless (empty if there is nothing below),
/// like the game which creates it before decoding its content.
fn put(out: &mut BTreeMap<String, Arc<[u8]>>, key: String, bytes: Arc<[u8]>) {
    if !is_tag(&key) {
        out.insert(key, bytes);
        return;
    }
    let Ok(new) = serde_json::from_slice::<Value>(&bytes) else {
        eprintln!("singleplayer: tag {key} unreadable, ignored");
        return;
    };
    let Some(add) = tag_values(&new) else {
        eprintln!("singleplayer: tag {key} without valid values, ignored");
        out.entry(key).or_insert_with(|| Arc::from(&br#"{"values":[]}"#[..]));
        return;
    };
    let replace = new.get("replace").and_then(Value::as_bool) == Some(true);
    let mut values: Vec<Value> = if replace {
        Vec::new()
    } else {
        out.get(&key)
            .and_then(|b| serde_json::from_slice::<Value>(b).ok())
            .and_then(|v| v.get("values").and_then(Value::as_array).cloned())
            .unwrap_or_default()
    };
    values.extend(add.iter().cloned());
    let merged = serde_json::to_vec(&serde_json::json!({ "values": values })).unwrap_or_default();
    out.insert(key, merged.into());
}

/// Files of a folder pack, paths relative to its root (`/` everywhere).
fn dir_files(root: &Path) -> std::vec::IntoIter<(String, Arc<[u8]>)> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path();
            let r = format!("{rel}{name}");
            if path.is_dir() {
                stack.push((path, format!("{r}/")));
            } else if (r.ends_with(".json") || r.ends_with(".mcfunction"))
                && let Ok(b) = std::fs::read(&path)
            {
                out.push((r, b.into()));
            }
        }
    }
    out.into_iter()
}

fn zip_files(path: &Path) -> std::vec::IntoIter<(String, Arc<[u8]>)> {
    let mut out = Vec::new();
    let Ok(f) = std::fs::File::open(path) else { return out.into_iter() };
    let Ok(mut z) = zip::ZipArchive::new(f) else { return out.into_iter() };
    for i in 0..z.len() {
        let Ok(mut e) = z.by_index(i) else { continue };
        let Ok(name) = e.name().map(|n| n.into_owned()) else { continue };
        if !(name.ends_with(".json") || name.ends_with(".mcfunction")) {
            continue;
        }
        let mut v = Vec::with_capacity(e.size() as usize);
        if e.read_to_end(&mut v).is_ok() {
            out.push((name, v.into()));
        }
    }
    out.into_iter()
}

fn zip_entry(path: &Path, name: &str) -> Option<Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::fs::File::open(path).ok()?).ok()?;
    let mut e = z.by_name(name).ok()?;
    let mut v = Vec::new();
    e.read_to_end(&mut v).ok()?;
    Some(v)
}

/// Tag entry: id (without `#`), reference to a tag, required.
struct Entry {
    id: String,
    tag: bool,
    required: bool,
}

fn entry_of(v: &Value) -> Option<Entry> {
    let (s, required) = match v {
        Value::String(s) => (s.as_str(), true),
        Value::Object(o) => (o.get("id")?.as_str()?, o.get("required").and_then(Value::as_bool).unwrap_or(true)),
        _ => return None,
    };
    let (tag, id) = match s.strip_prefix('#') {
        Some(t) => (true, t),
        None => (false, s),
    };
    Some(Entry { id: id_of(id), tag, required })
}

/// Tags resolved like the game: each tag flattened (references followed, duplicates
/// removed, first kept); a missing required element (`exists` false) or a required tag
/// that is absent, refused or not yet built makes the whole tag refused (log); a
/// missing optional entry is skipped. `raw`: tag → stacked `values` list.
///
/// Build order of the game: graph of references between tags, required edges of
/// all the tags first then optional ones, each discarded if it would close a cycle;
/// then depth-first traversal (dependencies before the tag) and a single build per
/// tag, each reference read in the tags already built. An optional
/// reference that closed a cycle is therefore skipped, and the tag that the other depends on in a
/// required way is built first. The game walks a hash table; we walk the sorted
/// names: the only difference, when several edges of the same kind close the same cycle, is which one
/// is discarded (difference reported in mission G3-F03).
pub fn resolve_tags<T: Clone + PartialEq>(
    raw: &BTreeMap<String, &[Value]>,
    exists: &dyn Fn(&str) -> Option<T>,
) -> BTreeMap<String, Vec<T>> {
    let entries: BTreeMap<&str, Vec<Option<Entry>>> =
        raw.iter().map(|(n, v)| (n.as_str(), v.iter().map(entry_of).collect())).collect();
    // Does `to` reach `from`? (Marked traversal: same answer as the game's recursion, without
    // looping back on a tag that cites itself.)
    fn reaches(deps: &BTreeMap<String, Vec<String>>, to: &str, from: &str, seen: &mut Vec<String>) -> bool {
        if seen.iter().any(|s| s == to) {
            return false;
        }
        seen.push(to.to_owned());
        deps.get(to).is_some_and(|d| d.iter().any(|x| x == from) || d.iter().any(|x| reaches(deps, x, from, seen)))
    }
    let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for required in [true, false] {
        for (name, list) in &entries {
            for e in list.iter().flatten().filter(|e| e.tag && e.required == required) {
                // Target without dependency (common case): no cycle possible, except itself.
                let cyclic = deps.contains_key(&e.id) && reaches(&deps, &e.id, name, &mut Vec::new());
                let d = deps.entry((*name).to_owned()).or_default();
                if !cyclic && !d.contains(&e.id) {
                    d.push(e.id.clone());
                }
            }
        }
    }
    fn visit<'a>(
        name: &'a str,
        deps: &'a BTreeMap<String, Vec<String>>,
        seen: &mut std::collections::BTreeSet<&'a str>,
        order: &mut Vec<&'a str>,
    ) {
        if !seen.insert(name) {
            return;
        }
        for d in deps.get(name).into_iter().flatten() {
            visit(d, deps, seen, order);
        }
        order.push(name);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut order = Vec::new();
    for name in entries.keys() {
        visit(name, &deps, &mut seen, &mut order);
    }
    let mut out: BTreeMap<String, Vec<T>> = BTreeMap::new();
    for name in order {
        let Some(list) = entries.get(name) else { continue };
        let mut vals: Vec<T> = Vec::new();
        let mut missing = Vec::new();
        for (e, v) in list.iter().zip(raw[name].iter()) {
            let Some(e) = e else {
                missing.push(v.to_string());
                continue;
            };
            let found = if e.tag {
                out.get(&e.id).map(|ids| {
                    for i in ids {
                        if !vals.contains(i) {
                            vals.push(i.clone());
                        }
                    }
                })
            } else {
                exists(&e.id).map(|i| {
                    if !vals.contains(&i) {
                        vals.push(i);
                    }
                })
            };
            if found.is_none() && e.required {
                missing.push(if e.tag { format!("#{}", e.id) } else { e.id.clone() });
            }
        }
        if missing.is_empty() {
            out.insert(name.to_owned(), vals);
        } else {
            eprintln!("singleplayer: tag {name} refused, missing references: {}", missing.join(", "));
        }
    }
    out
}

/// `values` lists of the tags `data/<namespace>/tags/<folder>/**.json` of the stack, by name.
pub(crate) fn raw_tags(jar: &Jar, dir: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for (name, bytes) in &jar.files {
        let Some(rest) = name.strip_prefix("data/") else { continue };
        let Some((ns, path)) = rest.split_once('/') else { continue };
        let Some(t) = path.strip_prefix("tags/").and_then(|p| p.strip_prefix(dir)).and_then(|p| p.strip_prefix('/')) else {
            continue;
        };
        let Some(t) = t.strip_suffix(".json") else { continue };
        // Invalid content: empty tag (the stack, `put`, already brought it back to this case).
        if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
            out.push((format!("{ns}:{t}"), Value::Array(tag_values(&v).cloned().unwrap_or_default())));
        }
    }
    out
}

/// Backup copy offered when opening a world with unstable settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unstable {
    /// Old "customized" world (`legacy_custom_options`).
    Customized,
    /// Experimental settings: non-vanilla dimensions, or registry data coming from a pack
    /// of the world.
    Experimental,
}

impl Unstable {
    /// Suffix of the `selectWorld.backupQuestion.*` / `backupWarning.*` keys.
    pub fn key(self) -> &'static str {
        match self {
            Unstable::Customized => "customized",
            Unstable::Experimental => "experimental",
        }
    }
}

/// Folders of the registries loaded from the packs with the world, outside `worldgen/…` (all counted).
const REGISTRY_DIRS: [&str; 34] = [
    "dimension_type",
    "chat_type",
    "trim_pattern",
    "trim_material",
    "trial_spawner",
    "wolf_variant",
    "wolf_sound_variant",
    "pig_variant",
    "pig_sound_variant",
    "frog_variant",
    "cat_variant",
    "cat_sound_variant",
    "cow_variant",
    "cow_sound_variant",
    "sulfur_cube_archetype",
    "chicken_variant",
    "chicken_sound_variant",
    "zombie_nautilus_variant",
    "painting_variant",
    "damage_type",
    "banner_pattern",
    "enchantment",
    "enchantment_provider",
    "jukebox_song",
    "instrument",
    "test_environment",
    "test_instance",
    "dialog",
    "world_clock",
    "timeline",
    "villager_trade",
    "trade_set",
    "decorated_pot_pattern",
    "block_transformer",
];

/// Dimensions provided by the game.
const BUILTIN_DIMS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

/// Does the game offer a backup before opening this world, and which one? Like it does:
/// old customized world, otherwise unstable settings — a dimension that does not have the vanilla
/// shape (type, generator, biome source; extra dimension), or a registry entry
/// (world generation, variants, enchantments…) coming from an enabled `datapacks` folder pack
/// (only the game's packs are deemed stable; feature ones too).
pub fn backup_question(world: &Path) -> Option<Unstable> {
    let wgs = crate::nbtio::read_dat(&world.join("data").join("minecraft").join("world_gen_settings.dat")).ok();
    let settings = wgs.as_ref().and_then(|r| r.get("data"));
    if settings.is_some_and(|d| d.get("legacy_custom_options").is_some()) {
        return Some(Unstable::Customized);
    }
    // Game dimensions missing from the saved settings: the dimension registry starts unstable
    // if one of them is brought by no pack (the game's pack brings none).
    let mut missing: Vec<&str> = Vec::new();
    if let Some(Nbt::Compound(dims)) = settings.and_then(|d| d.get("dimensions")) {
        if dims.iter().any(|(k, d)| !vanilla_like(k, d)) {
            return Some(Unstable::Experimental);
        }
        missing = BUILTIN_DIMS.into_iter().filter(|b| !dims.iter().any(|(k, _)| k.as_str() == *b)).collect();
    }
    let level = crate::storage::read_level(world, false).ok();
    let (enabled, _, _) = config_of(level.as_ref().and_then(|l| l.get("Data")));
    let avail = with_world(&[], world);
    let packs: Vec<&Pack> =
        enabled.iter().filter_map(|id| avail.iter().find(|p| p.id == *id && p.source == Source::World)).collect();
    let unprovided = missing.iter().any(|dim| {
        let rel = format!("data/minecraft/dimension/{}.json", dim.trim_start_matches("minecraft:"));
        !packs.iter().any(|p| has_file(p, &rel))
    });
    (unprovided || packs.into_iter().any(registry_data)).then_some(Unstable::Experimental)
}

/// Does the pack contain the file `rel` (root or overlay)?
fn has_file(p: &Pack, rel: &str) -> bool {
    let names = || std::iter::once(rel.to_owned()).chain(p.overlays.iter().map(|o| format!("{o}/{rel}")));
    match &p.loc {
        Loc::Dir(dir) => names().any(|n| dir.join(n).is_file()),
        Loc::Zip(z) => {
            let Ok(f) = std::fs::File::open(z) else { return false };
            let Ok(a) = zip::ZipArchive::new(f) else { return false };
            names().any(|n| a.index_for_name(&n).is_some())
        }
        Loc::Vanilla | Loc::Jar(_) => false,
    }
}

/// Vanilla-shaped dimension: normal world (surface or caves type; noise biome source
/// only with the normal world's parameter set), the game's Nether and End.
fn vanilla_like(key: &str, d: &Nbt) -> bool {
    let s = |n: Option<&Nbt>| n.and_then(Nbt::as_str).map(id_of);
    let ty = s(d.get("type"));
    let generator = d.get("generator");
    let gen_type = s(generator.and_then(|g| g.get("type")));
    let settings = s(generator.and_then(|g| g.get("settings")));
    let source = generator.and_then(|g| g.get("biome_source"));
    let source_type = s(source.and_then(|b| b.get("type")));
    let preset = s(source.and_then(|b| b.get("preset")));
    let noise = gen_type.as_deref() == Some("minecraft:noise");
    match key {
        "minecraft:overworld" => {
            matches!(ty.as_deref(), Some("minecraft:overworld" | "minecraft:overworld_caves"))
                && (source_type.as_deref() != Some("minecraft:multi_noise")
                    || preset.as_deref() == Some("minecraft:overworld"))
        }
        "minecraft:the_nether" => {
            ty.as_deref() == Some("minecraft:the_nether")
                && noise
                && settings.as_deref() == Some("minecraft:nether")
                && source_type.as_deref() == Some("minecraft:multi_noise")
                && preset.as_deref() == Some("minecraft:nether")
        }
        "minecraft:the_end" => {
            ty.as_deref() == Some("minecraft:the_end")
                && noise
                && settings.as_deref() == Some("minecraft:end")
                && source_type.as_deref() == Some("minecraft:the_end")
        }
        _ => false,
    }
}

/// Does the pack bring registry data (`data/` folder or an applicable
/// overlay)? Only the file names are read.
fn registry_data(p: &Pack) -> bool {
    let is_registry = |rel: &str| {
        let rest = rel.strip_prefix("data/").or_else(|| {
            p.overlays.iter().find_map(|o| rel.strip_prefix(o.as_str()).and_then(|r| r.strip_prefix("/data/")))
        });
        let Some((ns, path)) = rest.and_then(|r| r.split_once('/')) else { return false };
        if !path.ends_with(".json") {
            return false;
        }
        if let Some(dim) = path.strip_prefix("dimension/") {
            let key = format!("{ns}:{}", dim.trim_end_matches(".json"));
            return !BUILTIN_DIMS.contains(&key.as_str());
        }
        path.strip_prefix("worldgen/").is_some_and(|r| r.contains('/'))
            || REGISTRY_DIRS.iter().any(|d| path.strip_prefix(d).is_some_and(|r| r.starts_with('/')))
    };
    match &p.loc {
        Loc::Dir(dir) => {
            let mut stack = vec![(dir.clone(), String::new())];
            while let Some((d, rel)) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else { continue };
                for e in rd.flatten() {
                    let r = format!("{rel}{}", e.file_name().to_string_lossy());
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push((e.path(), format!("{r}/")));
                    } else if is_registry(&r) {
                        return true;
                    }
                }
            }
            false
        }
        Loc::Zip(z) => {
            let Ok(f) = std::fs::File::open(z) else { return false };
            let Ok(a) = zip::ZipArchive::new(f) else { return false };
            a.file_names().any(|n| n.is_ok_and(|n| is_registry(&n)))
        }
        Loc::Vanilla | Loc::Jar(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(id: &str, source: Source, features: &[&str]) -> Pack {
        Pack {
            id: id.into(),
            source,
            description: Value::String(String::new()),
            features: features.iter().map(|s| (*s).to_owned()).collect(),
            overlays: Vec::new(),
            filter: None,
            loc: Loc::Vanilla,
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    fn c(v: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(v.into_iter().map(|(k, x)| (k.to_owned(), x)).collect())
    }

    fn st(v: &str) -> Nbt {
        Nbt::String(v.into())
    }

    fn dim(ty: &str, generator: &str, settings: &str, source: Nbt) -> Nbt {
        c(vec![
            ("type", st(ty)),
            ("generator", c(vec![("type", st(generator)), ("settings", st(settings)), ("biome_source", source)])),
        ])
    }

    fn vanilla_dims() -> Vec<(&'static str, Nbt)> {
        let mn = |p: &str| c(vec![("type", st("minecraft:multi_noise")), ("preset", st(p))]);
        vec![
            ("minecraft:overworld", dim("minecraft:overworld", "minecraft:noise", "minecraft:overworld", mn("minecraft:overworld"))),
            ("minecraft:the_nether", dim("minecraft:the_nether", "minecraft:noise", "minecraft:nether", mn("minecraft:nether"))),
            (
                "minecraft:the_end",
                dim("minecraft:the_end", "minecraft:noise", "minecraft:end", c(vec![("type", st("minecraft:the_end"))])),
            ),
        ]
    }

    /// Test world: generation settings (`settings`) and `Data` of level.dat.
    fn world(dir: &Path, settings: Nbt, data: Nbt) {
        let enc = |n: Nbt| crate::nbtio::gzip(&crate::nbtio::encode_root(&n));
        std::fs::create_dir_all(dir.join("data").join("minecraft")).expect("folder");
        std::fs::write(dir.join("data/minecraft/world_gen_settings.dat"), enc(c(vec![("data", settings)]))).expect("dat");
        std::fs::write(dir.join("level.dat"), enc(c(vec![("Data", data)]))).expect("dat");
    }

    /// Copy offered on opening like the game: old customized world, non-vanilla dimension,
    /// registry entry of an enabled world pack; nothing for a vanilla world nor for
    /// a pack of functions alone or not enabled.
    #[test]
    fn backup_question_like_the_game() {
        let root = std::env::temp_dir().join(format!("rc-backupq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let enabled = |v: &[&str]| {
            c(vec![(
                "DataPacks",
                c(vec![
                    ("Enabled", Nbt::List(v.iter().map(|x| st(x)).collect())),
                    ("Disabled", Nbt::List(Vec::new())),
                ]),
            )])
        };
        let plain = root.join("vanilla");
        world(&plain, c(vec![("dimensions", c(vanilla_dims()))]), c(vec![]));
        assert_eq!(backup_question(&plain), None);

        let extra = root.join("extra");
        let mut dims = vanilla_dims();
        dims.push(("mod:mine", dims[0].1.clone()));
        world(&extra, c(vec![("dimensions", c(dims))]), c(vec![]));
        assert_eq!(backup_question(&extra), Some(Unstable::Experimental));

        let params = root.join("params");
        let mut dims = vanilla_dims();
        dims[0].1 = dim("minecraft:overworld", "minecraft:noise", "minecraft:overworld", c(vec![("type", st("minecraft:multi_noise"))]));
        world(&params, c(vec![("dimensions", c(dims))]), c(vec![]));
        assert_eq!(backup_question(&params), Some(Unstable::Experimental), "biome parameters written in full");

        let old = root.join("old");
        world(&old, c(vec![("dimensions", c(vanilla_dims())), ("legacy_custom_options", st("{}"))]), c(vec![]));
        assert_eq!(backup_question(&old), Some(Unstable::Customized));

        let meta = br#"{"pack":{"description":"x","pack_format":1}}"#;
        let packed = root.join("packed");
        world(&packed, c(vec![("dimensions", c(vanilla_dims()))]), enabled(&["vanilla", "file/p"]));
        let p = packed.join("datapacks").join("p");
        std::fs::create_dir_all(p.join("data/x/function")).expect("folder");
        std::fs::write(p.join("pack.mcmeta"), meta).expect("meta");
        std::fs::write(p.join("data/x/function/f.mcfunction"), b"say hi").expect("function");
        assert_eq!(backup_question(&packed), None, "functions alone: stable");
        std::fs::create_dir_all(p.join("data/x/worldgen/biome")).expect("folder");
        std::fs::write(p.join("data/x/worldgen/biome/b.json"), b"{}").expect("biome");
        assert_eq!(backup_question(&packed), Some(Unstable::Experimental));
        world(&packed, c(vec![("dimensions", c(vanilla_dims()))]), enabled(&["vanilla"]));
        assert_eq!(backup_question(&packed), None, "pack not enabled");

        // End missing from the settings: unstable, unless an enabled world pack brings it (the old
        // code only judged the dimensions present).
        let no_end = root.join("no_end");
        let dims: Vec<_> = vanilla_dims().into_iter().take(2).collect();
        world(&no_end, c(vec![("dimensions", c(dims.clone()))]), enabled(&["vanilla", "file/e"]));
        assert_eq!(backup_question(&no_end), Some(Unstable::Experimental), "End missing");
        let e = no_end.join("datapacks").join("e");
        std::fs::create_dir_all(e.join("data/minecraft/dimension")).expect("folder");
        std::fs::write(e.join("pack.mcmeta"), meta).expect("meta");
        std::fs::write(e.join("data/minecraft/dimension/the_end.json"), b"{}").expect("dimension");
        assert_eq!(backup_question(&no_end), None, "End brought by a world pack");
        world(&no_end, c(vec![("dimensions", c(dims))]), enabled(&["vanilla"]));
        assert_eq!(backup_question(&no_end), Some(Unstable::Experimental), "pack not enabled");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn repo() -> Vec<Pack> {
        let mut v = vec![
            pack("vanilla", Source::BuiltIn, &["minecraft:vanilla"]),
            pack("trade_rebalance", Source::Feature, &["minecraft:trade_rebalance"]),
            pack("minecart_improvements", Source::Feature, &["minecraft:minecart_improvements"]),
            pack("file/a", Source::World, &[]),
            pack("file/b", Source::World, &["minecraft:trade_rebalance"]),
        ];
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    #[test]
    fn new_world_packs_are_added_and_feature_packs_stay_off() {
        let sel = select(&repo(), &s(&["vanilla"]), &s(&[]), &s(&["minecraft:vanilla"]));
        // file/a new: added automatically; file/b asks for a missing flag: not loaded.
        assert_eq!(sel.enabled, s(&["vanilla", "file/a"]));
        assert_eq!(sel.disabled, s(&["file/b", "minecart_improvements", "trade_rebalance"]));
        assert_eq!(sel.features, s(&["minecraft:vanilla"]));
    }

    #[test]
    fn experiments_union_flags_and_force_their_pack() {
        // World created with the trade rebalance, pack absent from Enabled: imposed.
        let flags = s(&["minecraft:vanilla", "minecraft:trade_rebalance"]);
        let sel = select(&repo(), &s(&["vanilla"]), &s(&["file/a", "file/b"]), &flags);
        assert_eq!(sel.enabled, s(&["vanilla", "trade_rebalance"]));
        assert_eq!(sel.features, flags);
        // Flag already brought by a world pack added automatically: nothing imposed.
        let sel = select(&repo(), &s(&["vanilla"]), &s(&["file/a"]), &flags);
        assert_eq!(sel.enabled, s(&["vanilla", "file/b"]));
        assert_eq!(sel.features, flags);
        // Chosen pack with a flag missing from the world: removed.
        let sel = select(&repo(), &s(&["vanilla", "file/b"]), &s(&["file/a"]), &s(&["minecraft:vanilla"]));
        assert_eq!(sel.enabled, s(&["vanilla"]));
    }

    #[test]
    fn order_of_enabled_is_kept_and_empty_falls_back_to_vanilla() {
        let sel = select(&repo(), &s(&["file/a", "missing", "vanilla"]), &s(&["file/b"]), &s(&["minecraft:vanilla"]));
        assert_eq!(sel.enabled, s(&["file/a", "vanilla"]));
        let sel = select(&repo(), &s(&[]), &s(&["file/a", "file/b", "vanilla"]), &s(&["minecraft:vanilla"]));
        assert_eq!(sel.enabled, s(&["vanilla"]));
    }

    #[test]
    fn config_defaults_and_write_back_omit_defaults() {
        let (e, d, f) = config_of(None);
        assert_eq!((e, d, f), (s(&["vanilla"]), s(&[]), s(&["minecraft:vanilla"])));
        let mut data = Nbt::Compound(Vec::new());
        let sel = Selection { enabled: s(&["vanilla"]), disabled: Vec::new(), features: s(&["minecraft:vanilla"]) };
        write_config(&mut data, &sel);
        assert!(data.get("DataPacks").is_none() && data.get("enabled_features").is_none());
        let sel = Selection {
            enabled: s(&["vanilla", "trade_rebalance"]),
            disabled: s(&["file/a"]),
            features: s(&["minecraft:vanilla", "minecraft:trade_rebalance"]),
        };
        write_config(&mut data, &sel);
        assert_eq!(config_of(Some(&data)), (sel.enabled.clone(), sel.disabled.clone(), sel.features.clone()));
    }

    #[test]
    fn meta_needs_a_description_and_known_flags() {
        assert!(parse_meta(br#"{"pack":{"pack_format":121}}"#).is_none());
        assert!(parse_meta(br#"{"pack":{"description":"x"},"features":{"enabled":["minecraft:nope"]}}"#).is_none());
        let m = parse_meta(br#"{"pack":{"description":"x","min_format":121,"max_format":121},"features":{"enabled":["trade_rebalance"]}}"#).unwrap();
        assert_eq!(m.features, s(&["minecraft:trade_rebalance"]));
        let m = parse_meta(
            br#"{"pack":{"description":"x"},"overlays":{"entries":[
                {"directory":"old","formats":[10,20]},
                {"directory":"now","min_format":121,"max_format":[121,5]},
                {"directory":"later","min_format":[122],"max_format":200}]}}"#,
        )
        .unwrap();
        assert_eq!(m.overlays, s(&["now"]));
        assert!(parse_meta(br#"{"pack":{"description":"x"},"overlays":{"entries":[{"directory":"a","min_format":3}]}}"#).is_none());
    }

    #[test]
    fn stack_replaces_files_merges_tags_and_applies_overlays() {
        let mut jar = Jar {
            files: [
                ("data/minecraft/recipe/a.json", r#"{"v":0}"#),
                ("data/minecraft/tags/block/t.json", r#"{"values":["minecraft:stone"]}"#),
                ("data/minecraft/tags/block/u.json", r#"{"values":["minecraft:dirt"]}"#),
                ("data/minecraft/datapacks/trade_rebalance/data/minecraft/recipe/a.json", r#"{"v":1}"#),
                ("data/minecraft/datapacks/trade_rebalance/data/minecraft/tags/block/t.json", r#"{"values":["minecraft:sand"]}"#),
                ("data/minecraft/datapacks/trade_rebalance/now/data/minecraft/recipe/b.json", r#"{"v":2}"#),
                ("data/minecraft/datapacks/trade_rebalance/data/minecraft/tags/block/u.json", r#"{"replace":true,"values":["minecraft:gravel"]}"#),
            ]
            .iter()
            .map(|(k, v)| ((*k).to_owned(), Arc::from(v.as_bytes())))
            .collect(),
        };
        let mut tr = pack("trade_rebalance", Source::Feature, &["minecraft:trade_rebalance"]);
        tr.loc = Loc::Jar(format!("{BUILTIN_DIR}trade_rebalance/"));
        tr.overlays = s(&["now"]);
        let avail = vec![tr, pack("vanilla", Source::BuiltIn, &["minecraft:vanilla"])];
        let sel = Selection { enabled: s(&["vanilla", "trade_rebalance"]), disabled: Vec::new(), features: Vec::new() };
        stack(&mut jar, &avail, &sel);
        let get = |k: &str| serde_json::from_slice::<Value>(&jar.files[k]).unwrap();
        assert_eq!(get("data/minecraft/recipe/a.json")["v"], 1);
        assert_eq!(get("data/minecraft/recipe/b.json")["v"], 2);
        assert_eq!(get("data/minecraft/tags/block/t.json")["values"], serde_json::json!(["minecraft:stone", "minecraft:sand"]));
        assert_eq!(get("data/minecraft/tags/block/u.json")["values"], serde_json::json!(["minecraft:gravel"]));
        assert!(!jar.files.keys().any(|k| k.starts_with(BUILTIN_DIR)));
        // Reverse order: the base pack goes on top.
        let mut jar2 = Jar { files: [("data/minecraft/datapacks/trade_rebalance/data/minecraft/recipe/a.json".to_owned(), Arc::from(&br#"{"v":1}"#[..])), ("data/minecraft/recipe/a.json".to_owned(), Arc::from(&br#"{"v":0}"#[..]))].into_iter().collect() };
        let sel = Selection { enabled: s(&["trade_rebalance", "vanilla"]), disabled: Vec::new(), features: Vec::new() };
        stack(&mut jar2, &avail, &sel);
        assert_eq!(serde_json::from_slice::<Value>(&jar2.files["data/minecraft/recipe/a.json"]).unwrap()["v"], 0);
    }

    #[test]
    fn nested_tags_resolve_with_cycles_and_required_entries() {
        let vals = |j: &str| serde_json::from_str::<Value>(j).unwrap().as_array().unwrap().clone();
        let a = vals(r##"["t:f", "#t:b", "t:f"]"##);
        let b = vals(r##"["t:g", {"id":"#t:none","required":false}, {"id":"t:missing","required":false}]"##);
        let c = vals(r##"["#t:d"]"##);
        let d = vals(r##"["#t:c"]"##);
        let e = vals(r##"["t:missing"]"##);
        let f = vals(r##"["#t:e", "t:f"]"##);
        let raw: BTreeMap<String, &[Value]> = [("t:a", &a), ("t:b", &b), ("t:c", &c), ("t:d", &d), ("t:e", &e), ("t:uses_e", &f)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.as_slice()))
            .collect();
        let exists = |id: &str| (id == "t:f" || id == "t:g").then(|| id.to_owned());
        let r = resolve_tags(&raw, &exists);
        assert_eq!(r["t:a"], s(&["t:f", "t:g"]));
        assert_eq!(r["t:b"], s(&["t:g"]));
        // Cycle c <-> d, required element missing, tag depending on a refused one: refused.
        for k in ["t:c", "t:d", "t:e", "t:uses_e"] {
            assert!(!r.contains_key(k), "{k}");
        }
        // Cycle closed by an optional reference: it is discarded, the tag cited in a
        // required way is built first (whatever the order of the names).
        let opt_z = vals(r##"[{"id":"#t:z","required":false}, "t:f"]"##);
        let req_a = vals(r##"["#t:a"]"##);
        let opt_a = vals(r##"[{"id":"#t:a","required":false}, "t:f"]"##);
        let req_z = vals(r##"["#t:z"]"##);
        for (va, vz) in [(&opt_z, &req_a), (&req_z, &opt_a)] {
            let raw: BTreeMap<String, &[Value]> =
                [("t:a", va), ("t:z", vz)].into_iter().map(|(k, v)| (k.to_owned(), v.as_slice())).collect();
            let r = resolve_tags(&raw, &exists);
            assert_eq!(r.get("t:a").map(Vec::as_slice), Some(&s(&["t:f"])[..]));
            assert_eq!(r.get("t:z").map(Vec::as_slice), Some(&s(&["t:f"])[..]));
        }
        // Tag citing itself, cited by another: refused.
        let selfref = vals(r##"["#t:s", "t:f"]"##);
        let user = vals(r##"["#t:s"]"##);
        let raw: BTreeMap<String, &[Value]> =
            [("t:s", &selfref), ("t:u", &user)].into_iter().map(|(k, v)| (k.to_owned(), v.as_slice())).collect();
        assert!(resolve_tags(&raw, &exists).is_empty());
    }
}
