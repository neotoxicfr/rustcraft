//! Table of the sound events from `sounds.json` (spec § 2.2) and the draw of variants
//! (§ 3.6).

use std::sync::{Arc, OnceLock};

use hashbrown::{HashMap, HashSet};

use rc_assets::Assets;
use serde_json::Value;

use crate::rng::JavaRandom;

#[derive(Debug, Clone, PartialEq)]
pub enum VariantKind {
    /// Resource `<namespace>/sounds/<path>.ogg`, full path in the sources (`assets/…`).
    File(String),
    /// Redirect to another event (full identifier `namespace:path`).
    Event(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub kind: VariantKind,
    pub volume: f32,
    pub pitch: f32,
    pub weight: u32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: u32,
}

#[derive(Debug, Clone, Default)]
pub struct Event {
    pub variants: Vec<Variant>,
    pub subtitle: Option<String>,
}

/// Drawn variant, redirects resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// Path in the sources (`assets/minecraft/sounds/….ogg`).
    pub path: String,
    pub volume: f32,
    pub pitch: f32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: u32,
}

/// Variant drawn without allocation (`RUSTCRAFT_OPT_SOUNDIDS`): file designated by its index
/// ([`SoundTable::file_path`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    pub file: u32,
    pub volume: f32,
    pub pitch: f32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation_distance: u32,
}

/// Maximum depth of the redirects followed (beyond it: zero weight, nothing to play).
const MAX_DEPTH: u32 = 16;

#[derive(Debug, Clone, Copy)]
enum Target {
    File(u32),
    Event(u32),
    /// Redirect to a missing event: zero weight.
    Missing,
}

#[derive(Debug)]
struct CVariant {
    target: Target,
    /// Declared weight (file).
    own: u32,
    /// Effective weight without a depth bound (redirect: total of the targeted event).
    eff: u32,
    volume: f32,
    pitch: f32,
    stream: bool,
    preload: bool,
    attenuation_distance: u32,
}

#[derive(Debug)]
struct CEvent {
    id: Arc<str>,
    /// Sum of the effective weights.
    total: u32,
    /// Longest chain of redirects under the event; `None`: cycle.
    height: Option<u32>,
    variants: Vec<CVariant>,
}

/// Compiled table: interned identifiers, precomputed weights.
#[derive(Debug, Default)]
struct Compiled {
    index: HashMap<Arc<str>, u32>,
    events: Vec<CEvent>,
    files: Vec<Arc<str>>,
}

#[derive(Debug, Default)]
pub struct SoundTable {
    events: HashMap<String, Event>,
    /// Built at the first indexed draw, forgotten at each merge.
    compiled: OnceLock<Compiled>,
}

/// Full `namespace:path` (default namespace `minecraft`).
pub fn full_id(id: &str) -> String {
    if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") }
}

/// Sound file name → resource path.
fn sound_path(name: &str) -> String {
    let (ns, path) = rc_assets::source::split_id(name);
    format!("assets/{ns}/sounds/{path}.ogg")
}

impl SoundTable {
    /// Reads `sounds.json` in each layer, from the lowest to the highest priority.
    /// `file` variants whose `.ogg` exists in no layer are discarded.
    pub fn load(layers: &[&Assets]) -> Self {
        let mut oggs = HashSet::new();
        let mut files = Vec::new();
        for a in layers {
            oggs.extend(a.list("assets/", ".ogg"));
            files.push(a.list("assets/", "/sounds.json"));
        }
        let mut table = SoundTable::default();
        for (a, names) in layers.iter().zip(&files) {
            for path in names {
                // assets/<namespace>/sounds.json only.
                let Some(ns) = path.strip_prefix("assets/").and_then(|p| p.strip_suffix("/sounds.json")) else {
                    continue;
                };
                if ns.contains('/') {
                    continue;
                }
                let Some(bytes) = a.read(path) else { continue };
                match serde_json::from_slice::<Value>(&bytes) {
                    Ok(v) => table.merge(ns, &v, &oggs),
                    Err(e) => eprintln!("[audio] {path} ignored: {e}"),
                }
            }
        }
        table
    }

    /// Merges a `sounds.json` file of namespace `ns`. Whole file ignored if invalid.
    pub fn merge(&mut self, ns: &str, v: &Value, oggs: &HashSet<String>) {
        let Some(obj) = v.as_object() else { return };
        let mut parsed = Vec::with_capacity(obj.len());
        for (key, entry) in obj {
            match parse_entry(ns, entry, oggs) {
                Some(e) => parsed.push((format!("{ns}:{key}"), e)),
                None => {
                    eprintln!("[audio] sounds.json of {ns} invalid ({key}), file ignored");
                    return;
                }
            }
        }
        self.compiled = OnceLock::new();
        for (id, (replace, subtitle, variants)) in parsed {
            let ev = self.events.entry(id).or_default();
            if replace {
                ev.variants.clear();
            }
            if subtitle.is_some() {
                ev.subtitle = subtitle;
            }
            ev.variants.extend(variants);
        }
    }

    pub fn get(&self, id: &str) -> Option<&Event> {
        self.events.get(id)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.events.keys().map(String::as_str)
    }

    /// All `file` variants marked `preload`.
    pub fn preloads(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .events
            .values()
            .flat_map(|e| &e.variants)
            .filter(|v| v.preload)
            .filter_map(|v| match &v.kind {
                VariantKind::File(p) => Some(p.clone()),
                VariantKind::Event(_) => None,
            })
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Weight of a variant: its own, or the sum of the targeted event for a redirect.
    fn weight(&self, v: &Variant, depth: u32) -> u32 {
        match &v.kind {
            VariantKind::File(_) => v.weight,
            VariantKind::Event(target) => {
                if depth > 16 {
                    return 0;
                }
                self.events.get(target).map_or(0, |e| e.variants.iter().map(|w| self.weight(w, depth + 1)).sum())
            }
        }
    }

    /// Draws a variant of event `id` (full) with `rng`. `None`: nothing to play.
    pub fn pick(&self, id: &str, rng: &mut JavaRandom) -> Option<Resolved> {
        self.pick_depth(id, rng, 0)
    }

    fn pick_depth(&self, id: &str, rng: &mut JavaRandom, depth: u32) -> Option<Resolved> {
        if depth > 16 {
            return None;
        }
        let ev = self.events.get(id)?;
        let total: u32 = ev.variants.iter().map(|v| self.weight(v, depth)).sum();
        if total == 0 {
            return None;
        }
        let mut i = rng.next_int_bounded(total.min(i32::MAX as u32) as i32);
        for v in &ev.variants {
            i -= self.weight(v, depth) as i32;
            if i < 0 {
                return match &v.kind {
                    VariantKind::File(path) => Some(Resolved {
                        path: path.clone(),
                        volume: v.volume,
                        pitch: v.pitch,
                        stream: v.stream,
                        preload: v.preload,
                        attenuation_distance: v.attenuation_distance,
                    }),
                    VariantKind::Event(target) => {
                        let mut r = self.pick_depth(target, rng, depth + 1)?;
                        r.volume *= v.volume;
                        r.pitch *= v.pitch;
                        r.stream |= v.stream;
                        Some(r)
                    }
                };
            }
        }
        None
    }

    /// Builds the compiled table right away (otherwise at the first indexed draw).
    pub fn prepare(&self) {
        self.compiled();
    }

    fn compiled(&self) -> &Compiled {
        self.compiled.get_or_init(|| self.compile())
    }

    /// Event indices in the sorted order of the identifiers (stable from one load to
    /// the next), interned file paths, redirect heights, effective weights.
    fn compile(&self) -> Compiled {
        let mut c = Compiled::default();
        let mut ids: Vec<&String> = self.events.keys().collect();
        ids.sort_unstable();
        for (i, id) in ids.iter().enumerate() {
            let id: Arc<str> = Arc::from(id.as_str());
            c.index.insert(id.clone(), i as u32);
            c.events.push(CEvent { id, total: 0, height: None, variants: Vec::new() });
        }
        let mut files: HashMap<&str, u32> = HashMap::new();
        for (i, id) in ids.iter().enumerate() {
            let mut variants = Vec::with_capacity(self.events[*id].variants.len());
            for v in &self.events[*id].variants {
                let target = match &v.kind {
                    VariantKind::File(p) => Target::File(*files.entry(p.as_str()).or_insert_with(|| {
                        c.files.push(Arc::from(p.as_str()));
                        (c.files.len() - 1) as u32
                    })),
                    VariantKind::Event(t) => c.index.get(t.as_str()).map_or(Target::Missing, |&e| Target::Event(e)),
                };
                variants.push(CVariant {
                    target,
                    own: v.weight,
                    eff: 0,
                    volume: v.volume,
                    pitch: v.pitch,
                    stream: v.stream,
                    preload: v.preload,
                    attenuation_distance: v.attenuation_distance,
                });
            }
            c.events[i].variants = variants;
        }
        let n = c.events.len();
        let (mut state, mut height) = (vec![0u8; n], vec![None; n]);
        for e in 0..n {
            Self::height(&c, e, &mut state, &mut height);
        }
        let mut total: Vec<Option<u32>> = vec![None; n];
        for (e, h) in height.into_iter().enumerate() {
            if h.is_some() {
                Self::total(&mut c, e, &mut total);
            }
            c.events[e].height = h;
        }
        c
    }

    /// Longest chain of redirects under `e`; `None` if a cycle is reachable.
    fn height(c: &Compiled, e: usize, state: &mut [u8], out: &mut [Option<u32>]) -> Option<u32> {
        match state[e] {
            2 => return out[e],
            1 => return None,
            _ => {}
        }
        state[e] = 1;
        let mut h = Some(0u32);
        for v in &c.events[e].variants {
            if let Target::Event(t) = v.target {
                let ht = Self::height(c, t as usize, state, out);
                h = h.zip(ht).map(|(a, b)| a.max(b.saturating_add(1)));
            }
        }
        state[e] = 2;
        out[e] = h;
        h
    }

    /// Effective weights (without a depth bound) of an event with no reachable cycle. Sum
    /// modulo 2³², like the original sum in the optimised version.
    fn total(c: &mut Compiled, e: usize, memo: &mut [Option<u32>]) -> u32 {
        if let Some(t) = memo[e] {
            return t;
        }
        let mut sum = 0u32;
        for k in 0..c.events[e].variants.len() {
            let eff = match c.events[e].variants[k].target {
                Target::File(_) => c.events[e].variants[k].own,
                Target::Missing => 0,
                Target::Event(t) => Self::total(c, t as usize, memo),
            };
            c.events[e].variants[k].eff = eff;
            sum = sum.wrapping_add(eff);
        }
        c.events[e].total = sum;
        memo[e] = Some(sum);
        sum
    }

    /// Index of event `id` (default namespace `minecraft`), without allocation.
    pub fn event_index(&self, id: &str) -> Option<u32> {
        const NS: &[u8] = b"minecraft:";
        let c = self.compiled();
        if id.contains(':') {
            return c.index.get(id).copied();
        }
        let mut buf = [0u8; 160];
        let n = NS.len() + id.len();
        if n > buf.len() {
            return c.index.get(full_id(id).as_str()).copied();
        }
        buf[..NS.len()].copy_from_slice(NS);
        buf[NS.len()..n].copy_from_slice(id.as_bytes());
        c.index.get(std::str::from_utf8(&buf[..n]).ok()?).copied()
    }

    /// Full identifier of the event of index `e`.
    pub fn event_id(&self, e: u32) -> &Arc<str> {
        &self.compiled().events[e as usize].id
    }

    /// Path in the sources of the file of index `f` (`assets/minecraft/sounds/….ogg`).
    pub fn file_path(&self, f: u32) -> &Arc<str> {
        &self.compiled().files[f as usize]
    }

    /// Same draw as [`pick`](Self::pick) (same generator calls, same variant), without
    /// allocation: weights precomputed as long as the bounded depth cannot change them.
    pub fn pick_index(&self, e: u32, rng: &mut JavaRandom) -> Option<Pick> {
        Self::cpick(self.compiled(), e, rng, 0)
    }

    /// [`pick_index`](Self::pick_index) of event `id` (default namespace `minecraft`).
    pub fn pick_fast(&self, id: &str, rng: &mut JavaRandom) -> Option<Pick> {
        self.pick_index(self.event_index(id)?, rng)
    }

    /// Weight of a variant at depth `depth`, computed like [`weight`](Self::weight).
    fn cweight(c: &Compiled, v: &CVariant, depth: u32) -> u32 {
        match v.target {
            Target::File(_) => v.own,
            Target::Missing => 0,
            Target::Event(t) => {
                if depth > MAX_DEPTH {
                    return 0;
                }
                c.events[t as usize].variants.iter().map(|w| Self::cweight(c, w, depth + 1)).sum()
            }
        }
    }

    fn cpick(c: &Compiled, e: u32, rng: &mut JavaRandom, depth: u32) -> Option<Pick> {
        if depth > MAX_DEPTH {
            return None;
        }
        let ev = &c.events[e as usize];
        // The deepest redirect under `e` is weighed at depth `depth + h - 1`: as long as
        // it does not exceed the bound, the weights are the unbounded, precomputed ones.
        let fast = ev.height.is_some_and(|h| depth.saturating_add(h) <= MAX_DEPTH + 1);
        let w = |v: &CVariant| if fast { v.eff } else { Self::cweight(c, v, depth) };
        let total: u32 = if fast { ev.total } else { ev.variants.iter().map(w).sum() };
        if total == 0 {
            return None;
        }
        let mut i = rng.next_int_bounded(total.min(i32::MAX as u32) as i32);
        for v in &ev.variants {
            i -= w(v) as i32;
            if i < 0 {
                return match v.target {
                    Target::File(file) => Some(Pick {
                        file,
                        volume: v.volume,
                        pitch: v.pitch,
                        stream: v.stream,
                        preload: v.preload,
                        attenuation_distance: v.attenuation_distance,
                    }),
                    Target::Event(t) => {
                        let mut r = Self::cpick(c, t, rng, depth + 1)?;
                        r.volume *= v.volume;
                        r.pitch *= v.pitch;
                        r.stream |= v.stream;
                        Some(r)
                    }
                    Target::Missing => None,
                };
            }
        }
        None
    }
}

type Entry = (bool, Option<String>, Vec<Variant>);

fn parse_entry(ns: &str, e: &Value, oggs: &HashSet<String>) -> Option<Entry> {
    let o = e.as_object()?;
    let replace = match o.get("replace") {
        None => false,
        Some(b) => b.as_bool()?,
    };
    let subtitle = match o.get("subtitle") {
        None => None,
        Some(s) => Some(s.as_str()?.to_owned()),
    };
    let mut variants = Vec::new();
    if let Some(list) = o.get("sounds") {
        for s in list.as_array()? {
            let v = parse_variant(ns, s)?;
            if let VariantKind::File(p) = &v.kind
                && !oggs.contains(p)
            {
                eprintln!("[audio] missing file: {p}");
                continue;
            }
            variants.push(v);
        }
    }
    Some((replace, subtitle, variants))
}

fn parse_variant(ns: &str, s: &Value) -> Option<Variant> {
    // Name without namespace: that of the sounds.json file.
    let qualify = |n: &str| if n.contains(':') { n.to_owned() } else { format!("{ns}:{n}") };
    let mut v = Variant {
        kind: VariantKind::File(String::new()),
        volume: 1.0,
        pitch: 1.0,
        weight: 1,
        stream: false,
        preload: false,
        attenuation_distance: 16,
    };
    if let Some(name) = s.as_str() {
        v.kind = VariantKind::File(sound_path(&qualify(name)));
        return Some(v);
    }
    let o = s.as_object()?;
    let name = qualify(o.get("name")?.as_str()?);
    let f = |k: &str, d: f32| o.get(k).map_or(Some(d), |x| x.as_f64().map(|x| x as f32));
    v.volume = f("volume", 1.0)?;
    v.pitch = f("pitch", 1.0)?;
    if v.volume <= 0.0 || v.pitch <= 0.0 {
        return None;
    }
    v.weight = o.get("weight").map_or(Some(1), |x| x.as_u64().map(|x| x as u32))?;
    if v.weight == 0 {
        return None;
    }
    v.stream = o.get("stream").map_or(Some(false), Value::as_bool)?;
    v.preload = o.get("preload").map_or(Some(false), Value::as_bool)?;
    v.attenuation_distance = o.get("attenuation_distance").map_or(Some(16), |x| x.as_u64().map(|x| x as u32))?;
    v.kind = match o.get("type").map_or(Some("file"), Value::as_str)? {
        "file" => VariantKind::File(sound_path(&name)),
        "event" => VariantKind::Event(name),
        _ => return None,
    };
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(json: &str) -> SoundTable {
        let oggs: HashSet<String> =
            ["a", "b", "c", "d"].iter().map(|n| format!("assets/minecraft/sounds/{n}.ogg")).collect();
        let mut t = SoundTable::default();
        t.merge("minecraft", &serde_json::from_str(json).unwrap(), &oggs);
        t
    }

    #[test]
    fn weighted_pick_matches_java_draw() {
        let t = table(r#"{ "x": { "sounds": [ "a", { "name": "b", "weight": 3 }, "missing" ] } }"#);
        // Missing file discarded: total weight 4.
        assert_eq!(t.get("minecraft:x").unwrap().variants.len(), 2);
        for seed in 0..200i64 {
            let i = JavaRandom::new(seed).next_int_bounded(4);
            let want = if i < 1 { "a" } else { "b" };
            let got = t.pick("minecraft:x", &mut JavaRandom::new(seed)).unwrap();
            assert_eq!(got.path, format!("assets/minecraft/sounds/{want}.ogg"));
        }
    }

    #[test]
    fn event_reference_weight_and_composition() {
        let t = table(
            r#"{ "base": { "sounds": [ { "name": "c", "volume": 0.5, "pitch": 2, "weight": 2 }, { "name": "d", "weight": 3, "attenuation_distance": 48 } ] },
                 "ref": { "sounds": [ "a", { "name": "base", "type": "event", "volume": 0.5, "pitch": 0.5, "weight": 100, "stream": true } ] } }"#,
        );
        // Weight of the redirect = 2 + 3 = 5 (not 100): total 6.
        for seed in 0..200i64 {
            let mut r = JavaRandom::new(seed);
            let i = r.next_int_bounded(6);
            let got = t.pick("minecraft:ref", &mut JavaRandom::new(seed)).unwrap();
            if i < 1 {
                assert!(got.path.ends_with("/a.ogg"));
                continue;
            }
            let j = r.next_int_bounded(5);
            if j < 2 {
                assert!(got.path.ends_with("/c.ogg"));
                assert_eq!((got.volume, got.pitch), (0.25, 1.0));
            } else {
                assert!(got.path.ends_with("/d.ogg"));
                assert_eq!(got.attenuation_distance, 48);
            }
            assert!(got.stream);
        }
    }

    /// FNV-1a fingerprint of all the draws (variant, settings, generator state afterwards), for
    /// each seed and each event, by the old draw (`pick`); the indexed draw must
    /// return exactly the same sequence.
    fn fingerprint(t: &SoundTable, seeds: i64) -> (u64, usize) {
        let mut ids: Vec<&str> = t.ids().collect();
        ids.sort_unstable();
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let mut mix = |x: u64| {
            for b in x.to_le_bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
        };
        let mut picked = 0;
        for id in &ids {
            for seed in 0..seeds {
                let (mut a, mut b) = (JavaRandom::new(seed), JavaRandom::new(seed));
                let old = t.pick(id, &mut a);
                let new = t.pick_fast(id, &mut b);
                let new_path = new.map(|p| t.file_path(p.file).to_string());
                assert_eq!(old.as_ref().map(|r| r.path.clone()), new_path, "{id} seed {seed}");
                if let (Some(o), Some(n)) = (&old, &new) {
                    let o_bits = (o.volume.to_bits(), o.pitch.to_bits(), o.stream, o.preload, o.attenuation_distance);
                    let n_bits = (n.volume.to_bits(), n.pitch.to_bits(), n.stream, n.preload, n.attenuation_distance);
                    assert_eq!(o_bits, n_bits, "{id} seed {seed}");
                }
                let after = a.next_int();
                assert_eq!(after, b.next_int(), "generator after the draw: {id} seed {seed}");
                if let Some(o) = &old {
                    picked += 1;
                    for b in o.path.bytes() {
                        mix(u64::from(b));
                    }
                    mix(u64::from(o.volume.to_bits()) << 32 | u64::from(o.pitch.to_bits()));
                    mix(u64::from(o.attenuation_distance) << 2 | u64::from(o.stream) << 1 | u64::from(o.preload));
                } else {
                    mix(0xdead);
                }
                mix(after as u64);
            }
        }
        (h, picked)
    }

    /// Chain of 22 redirects (crosses the depth bound 16), cycle, missing redirect, varied weights.
    fn deep_table() -> SoundTable {
        let mut o = serde_json::Map::new();
        for k in 0..22 {
            let name = ["a", "b", "c", "d"][k % 4];
            let mut sounds = vec![serde_json::json!({ "name": name, "weight": 1 + k % 5, "volume": 0.9 })];
            if k + 1 < 22 {
                sounds.push(serde_json::json!({ "name": format!("chain{}", k + 1), "type": "event", "pitch": 1.1, "stream": k % 3 == 0 }));
            }
            o.insert(format!("chain{k}"), serde_json::json!({ "sounds": sounds }));
        }
        let x = serde_json::json!({
            "cyc_a": { "sounds": [ "a", { "name": "cyc_b", "type": "event", "weight": 7 } ] },
            "cyc_b": { "sounds": [ { "name": "b", "weight": 2 }, { "name": "cyc_a", "type": "event" }, { "name": "c", "attenuation_distance": 30 } ] },
            "to_cyc": { "sounds": [ { "name": "cyc_a", "type": "event", "volume": 0.5 }, "d" ] },
            "missing": { "sounds": [ { "name": "nowhere", "type": "event", "weight": 50 }, "a" ] },
            "only_missing": { "sounds": [ { "name": "nowhere", "type": "event" } ] },
            "empty": { "sounds": [] },
            "self": { "sounds": [ { "name": "self", "type": "event" }, "b" ] },
        });
        for (k, v) in x.as_object().unwrap() {
            o.insert(k.clone(), v.clone());
        }
        let oggs: HashSet<String> =
            ["a", "b", "c", "d"].iter().map(|n| format!("assets/minecraft/sounds/{n}.ogg")).collect();
        let mut t = SoundTable::default();
        t.merge("minecraft", &Value::Object(o), &oggs);
        t
    }

    #[test]
    fn indexed_pick_matches_recursive_pick_synthetic() {
        let t = deep_table();
        assert_eq!(t.len(), 29);
        let (h, picked) = fingerprint(&t, 10_000);
        // Fingerprint of the old draw, stored: fails if either changes.
        assert_eq!((h, picked), (SYNTH_PRINT, SYNTH_PICKED), "fingerprint {h:#x}, {picked} draws");
        // Without a namespace: same event.
        assert_eq!(t.event_index("chain3"), t.event_index("minecraft:chain3"));
        assert!(t.event_index("minecraft:nowhere").is_none());
    }

    const SYNTH_PRINT: u64 = 0xea4c_8ae8_3293_7e74;
    const SYNTH_PICKED: usize = 265_243;

    /// All the events of the 26.3 game's `sounds.json` × 10,000 seeds.
    #[test]
    fn indexed_pick_matches_recursive_pick_vanilla() {
        let Some(install) = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game") else { return };
        let Some(index) = rc_testdata::need(install.asset_index(), "26.3 sound files and fonts (asset index)") else { return };
        let index = Some(index);
        let Some(assets) = rc_testdata::need(
            rc_assets::Assets::for_install(&install.client_jar, &[], index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())))
                .ok(),
            "26.3 game resources",
        ) else {
            return;
        };
        let t = SoundTable::load(&[&assets]);
        let (h, picked) = fingerprint(&t, 10_000);
        assert_eq!((t.len(), h, picked), (VANILLA_EVENTS, VANILLA_PRINT, VANILLA_PICKED), "fingerprint {h:#x}");
    }

    const VANILLA_EVENTS: usize = 1991;
    const VANILLA_PRINT: u64 = 0x0bdd_3d23_6e57_4334;
    const VANILLA_PICKED: usize = 19_840_000;

    #[test]
    fn replace_and_invalid_file() {
        let oggs: HashSet<String> =
            ["assets/minecraft/sounds/a.ogg".into(), "assets/minecraft/sounds/b.ogg".into()].into();
        let mut t = SoundTable::default();
        t.merge("minecraft", &serde_json::json!({ "x": { "sounds": ["a"], "subtitle": "s.a" } }), &oggs);
        t.merge("minecraft", &serde_json::json!({ "x": { "sounds": ["b"] } }), &oggs);
        assert_eq!(t.get("minecraft:x").unwrap().variants.len(), 2);
        t.merge("minecraft", &serde_json::json!({ "x": { "sounds": ["b"], "replace": true } }), &oggs);
        assert_eq!(t.get("minecraft:x").unwrap().variants.len(), 1);
        assert_eq!(t.get("minecraft:x").unwrap().subtitle.as_deref(), Some("s.a"));
        // Unknown type: the whole file is ignored.
        t.merge(
            "minecraft",
            &serde_json::json!({ "y": { "sounds": ["a"] }, "z": { "sounds": [{ "name": "a", "type": "?" }] } }),
            &oggs,
        );
        assert!(t.get("minecraft:y").is_none());
    }
}
