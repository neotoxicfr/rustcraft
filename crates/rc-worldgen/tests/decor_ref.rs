// Direct read allowed (test: settings read at launch, outside the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Block-by-block decoration against the Java probe (`worldgen-ref/decor/decor_*.bin` + `.trace`):
//! 3×3 chunks at status terrain, decoration of the central chunk alone (order neutralised).
//! `cargo test --release -p rc-worldgen --test decor_ref -- --nocapture`

#![allow(clippy::type_complexity, clippy::needless_range_loop)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rc_worldgen::decor::Decorator;
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_data::DecorData;
use rc_worldgen::decor_region::{DecoChunk, FINAL_HMS, Hm, Region};

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

struct Case {
    seed: i64,
    cx: i32,
    cz: i32,
    /// Bottom, height, sea level, generator bottom and depth.
    dim: [i32; 5],
    names: Vec<String>,
    before: Vec<DecoChunk>,
    before_final: Vec<Vec<i16>>,
    after: Vec<Vec<u16>>,
    after_final: Vec<Vec<i16>>,
}

struct Rd<'a> {
    b: &'a [u8],
    i: usize,
}

impl Rd<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        s
    }
    fn be_i64(&mut self) -> i64 {
        i64::from_be_bytes(self.take(8).try_into().unwrap())
    }
    fn be_i32(&mut self) -> i32 {
        i32::from_be_bytes(self.take(4).try_into().unwrap())
    }
    fn le_u16s(&mut self, n: usize) -> Vec<u16> {
        self.take(n * 2).as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect()
    }
    fn le_i16s(&mut self, n: usize) -> Vec<i16> {
        self.take(n * 2).as_chunks::<2>().0.iter().map(|c| i16::from_le_bytes(*c)).collect()
    }
}

fn read_case(path: &Path, source_index: &BTreeMap<String, u16>) -> Case {
    let data = std::fs::read(path).unwrap();
    let mut r = Rd { b: &data, i: 0 };
    let magic = r.take(4).to_vec();
    let dim = if magic == b"DEC2" {
        [r.be_i32(), r.be_i32(), r.be_i32(), r.be_i32(), r.be_i32()]
    } else {
        assert_eq!(magic, b"DEC1");
        [MIN_Y, HEIGHT, 63, MIN_Y, HEIGHT]
    };
    let size = 256 * dim[1] as usize;
    let seed = r.be_i64();
    let cx = r.be_i32();
    let cz = r.be_i32();
    let n = r.be_i32() as usize;
    let mut names = Vec::new();
    for _ in 0..n {
        let l = u16::from_be_bytes(r.take(2).try_into().unwrap()) as usize;
        names.push(String::from_utf8(r.take(l).to_vec()).unwrap());
    }
    let map: Vec<u16> = names.iter().map(|n| source_index.get(n).copied().unwrap_or(u16::MAX)).collect();
    let mut before = Vec::new();
    let mut before_final = Vec::new();
    for k in 0..9 {
        let (dx, dz) = (k % 3 - 1, k / 3 - 1);
        let mut c = DecoChunk::new(cx + dx, cz + dz, dim[0], dim[1]);
        c.blocks = r.le_u16s(size).into_iter().map(u32::from).collect();
        let wg = r.le_i16s(512);
        c.heights[Hm::WorldSurfaceWg as usize].copy_from_slice(&wg[..256]);
        c.heights[Hm::OceanFloorWg as usize].copy_from_slice(&wg[256..]);
        before_final.push(r.le_i16s(1024));
        c.biomes = r.take(size / 64).iter().map(|&i| map[i as usize]).collect();
        before.push(c);
    }
    let mut after = Vec::new();
    let mut after_final = Vec::new();
    for _ in 0..9 {
        after.push(r.le_u16s(size));
        after_final.push(r.le_i16s(1024));
    }
    assert_eq!(r.i, data.len());
    Case { seed, cx, cz, dim, names, before, before_final, after, after_final }
}

struct Trace {
    /// (name of the feature, changes (x, y, z, state)).
    steps: Vec<(String, Vec<([i32; 3], u32)>)>,
}

fn read_trace(path: &Path) -> Trace {
    let text = std::fs::read_to_string(path).unwrap();
    let mut steps: Vec<(String, Vec<([i32; 3], u32)>)> = Vec::new();
    for line in text.lines() {
        let mut it = line.split(' ');
        if line.starts_with("F ") {
            it.next();
            steps.push((it.next().unwrap().to_owned(), Vec::new()));
        } else {
            let v: Vec<i64> = it.map(|x| x.parse().unwrap()).collect();
            steps.last_mut().unwrap().1.push(([v[0] as i32, v[1] as i32, v[2] as i32], v[3] as u32));
        }
    }
    Trace { steps }
}

struct World {
    blocks: Blocks,
    data: DecorData,
    source_names: Vec<String>,
    nether_names: Vec<String>,
    end_names: Vec<String>,
}

fn world() -> Option<World> {
    let pack = common::data_pack()?;
    let jar = common::client_jar()?;
    let data = DecorData::from_jar(&jar).expect("decoration data");
    let v = rc_data::load_or_prepare("26.3", |m| eprintln!("rc-data : {m}")).expect("rc-data");
    let blocks = Blocks::new(&v, &data).expect("blocks");
    let (src, _) = rc_worldgen::biome::BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let (nether, _) = rc_worldgen::biome::BiomeSource::from_preset(&pack, "normal", "the_nether").unwrap();
    Some(World {
        blocks,
        data,
        source_names: src.names(),
        nether_names: nether.names(),
        end_names: rc_worldgen::biome::BiomeSource::TheEnd.names(),
    })
}

fn cases() -> Vec<PathBuf> {
    cases_with("decor_")
}

fn cases_with(prefix: &str) -> Vec<PathBuf> {
    let Some(dir) = common::ref_dir().map(|d| d.join("decor")) else { return Vec::new() };
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension().is_some_and(|e| e == "bin")
                        && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix))
                })
                .collect()
        })
        .unwrap_or_default();
    if let Ok(f) = std::env::var("DECOR_CASE") {
        v.retain(|p| p.to_string_lossy().contains(&f));
    }
    v.sort();
    v
}

fn region<'a>(chunks: &'a mut [DecoChunk], b: &'a Blocks, seed: i64, dim: [i32; 5]) -> Region<'a> {
    let arr: [&mut DecoChunk; 9] = {
        let mut it = chunks.iter_mut();
        std::array::from_fn(|_| it.next().unwrap())
    };
    Region::new(arr, b, dim[2], seed).with_generator(dim[3], dim[4])
}

/// Features whose differences would be tolerated (only reported). Empty: the iceberg differences
/// came from a badly discarded NaN, not from the last bit of sin/cos (angles checked against Java).
const KNOWN_TRIG: [&str; 0] = [];

/// Step-by-step comparison of a decoration with the game's trace.
struct Checker<'t> {
    trace: &'t Trace,
    expected: Vec<Vec<u32>>,
    cx: i32,
    cz: i32,
    min_y: i32,
    size: usize,
    step: usize,
    first_bad: Option<String>,
    bad_features: Vec<(String, usize)>,
}

impl Checker<'_> {
    fn step(&mut self, r: &mut Region, b: &Blocks, name: String) {
        let (tname, changes) = match self.trace.steps.get(self.step) {
            Some(t) => (t.0.clone(), t.1.clone()),
            None => ("<fin>".to_owned(), Vec::new()),
        };
        self.step += 1;
        if tname != name {
            if self.first_bad.is_none() {
                self.first_bad = Some(format!("order: us {name}, game {tname}"));
            }
            return;
        }
        for &(p, s) in changes.iter() {
            let k = (((p[2] >> 4) - self.cz + 1) * 3 + ((p[0] >> 4) - self.cx + 1)) as usize;
            let i = ((((p[1] - self.min_y) << 4) | (p[2] & 15)) << 4 | (p[0] & 15)) as usize;
            self.expected[k][i] = s;
        }
        let mut diffs = Vec::new();
        for (k, exp) in self.expected.iter().enumerate() {
            let c = &r.chunks[k];
            for i in 0..self.size {
                if c.blocks[i] != exp[i] {
                    diffs.push((k, i));
                }
            }
        }
        if diffs.is_empty() {
            return;
        }
        self.bad_features.push((name.clone(), diffs.len()));
        if self.first_bad.is_none() {
            let mut msg = format!("{name}: {} differences", diffs.len());
            let shown: usize = std::env::var("DECOR_SHOW").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
            for &(k, i) in diffs.iter().take(shown) {
                let c = &r.chunks[k];
                let (x, y, z) =
                    (c.cx * 16 + (i & 15) as i32, self.min_y + (i >> 8) as i32, c.cz * 16 + ((i >> 4) & 15) as i32);
                msg.push_str(&format!(
                    "\n    ({x},{y},{z}) us {} / game {}",
                    b.describe(c.blocks[i]),
                    b.describe(self.expected[k][i])
                ));
            }
            self.first_bad = Some(msg);
        }
        // Resynchronises on the game's state to isolate the next feature.
        for &(k, i) in &diffs {
            r.chunks[k].blocks[i] = self.expected[k][i];
        }
    }
}

/// Checks all the cases of a prefix; `force`: forced placement (feature and attempts read from the trace).
fn check_all(prefix: &str, force: bool) {
    let files = cases_with(prefix);
    if files.is_empty() {
        eprintln!("no case {prefix}: test skipped");
        return;
    }
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let names = if prefix.starts_with("nether_") {
        &w.nether_names
    } else if prefix.starts_with("end_") {
        &w.end_names
    } else {
        &w.source_names
    };
    let source_index: BTreeMap<String, u16> =
        names.iter().enumerate().map(|(i, n)| (rc_worldgen::data::full_id(n), i as u16)).collect();
    let mut total_bad = 0usize;
    let mut missing_all: BTreeMap<String, u32> = BTreeMap::new();
    let mut deco_cache: BTreeMap<i64, Decorator> = BTreeMap::new();
    for path in &files {
        let case = read_case(path, &source_index);
        let trace = read_trace(&path.with_extension("trace"));
        let deco = deco_cache.entry(case.seed).or_insert_with(|| {
            Decorator::new(&w.data, b, names, case.seed).expect("decorator").with_templates(&w.data, b)
        });
        let mut chunks = case.before;
        for (k, c) in chunks.iter_mut().enumerate() {
            c.prime_final_heightmaps(b);
            for (i, hm) in FINAL_HMS.iter().enumerate() {
                let theirs = &case.before_final[k][i * 256..(i + 1) * 256];
                assert_eq!(&c.heights[*hm as usize][..], theirs, "{}: map {hm:?} of chunk {k}", path.display());
            }
        }
        let mut ck = Checker {
            trace: &trace,
            expected: chunks.iter().map(|c| c.blocks.clone()).collect(),
            cx: case.cx,
            cz: case.cz,
            min_y: case.dim[0],
            size: 256 * case.dim[1] as usize,
            step: 0,
            first_bad: None,
            bad_features: Vec::new(),
        };
        let t0 = Instant::now();
        let stats = {
            let mut reg = region(&mut chunks, b, case.seed, case.dim);
            if force {
                let name = trace.steps.first().map(|s| s.0.clone()).unwrap_or_default();
                let n = trace.steps.len() as u32;
                deco.force_traced(&mut reg, &name, n, |r, f| ck.step(r, b, deco.reg.placed_name(f).to_owned()))
            } else {
                deco.decorate_traced(&mut reg, |r, _s, f| ck.step(r, b, deco.reg.placed_name(f).to_owned()))
            }
        };
        let dt = t0.elapsed();
        let mut final_bad = 0;
        for (k, c) in chunks.iter().enumerate() {
            for i in 0..c.blocks.len() {
                if c.blocks[i] != u32::from(case.after[k][i]) {
                    final_bad += 1;
                }
            }
        }
        let bad: usize = ck.bad_features.iter().map(|x| x.1).sum();
        total_bad += ck.bad_features.iter().filter(|x| !KNOWN_TRIG.contains(&x.0.as_str())).map(|x| x.1).sum::<usize>();
        for (k, v) in &stats.missing {
            *missing_all.entry(k.clone()).or_insert(0) += v;
        }
        eprintln!(
            "{}: {} features, {} differences ({} faulty features), final {final_bad}, {:.1} ms (with traces){}",
            path.file_name().unwrap().to_string_lossy(),
            stats.features_run,
            bad,
            ck.bad_features.len(),
            dt.as_secs_f64() * 1e3,
            if stats.missing.is_empty() { String::new() } else { format!(", missing {:?}", stats.missing) }
        );
        if !ck.bad_features.is_empty() {
            let mut agg: BTreeMap<String, (usize, usize)> = BTreeMap::new();
            for (n, c) in &ck.bad_features {
                let e = agg.entry(n.clone()).or_insert((0, 0));
                e.0 += 1;
                e.1 += c;
            }
            eprintln!("  faulty: {agg:?}");
        }
        if let Some(m) = ck.first_bad {
            eprintln!("  first: {m}");
        }
        let _ = (&case.names, &case.after_final);
    }
    eprintln!("set buckets that became trees (process total): {}", rc_worldgen::jset::treeified_count());
    eprintln!("total outside known limits: {total_bad} differences; missing {missing_all:?}");
    assert_eq!(total_bad, 0, "decoration differences");
}

#[test]
fn decoration_matches_reference() {
    check_all("decor_", false);
}

/// Nether (probe launched with `DECOR_DIM=nether`): bottom 0, height 256, generation over 128.
#[test]
fn nether_decoration_matches_reference() {
    check_all("nether_", false);
}

/// End (probe launched with `DECOR_DIM=end`): pillars, gateways, chorus, islands, platform.
#[test]
fn end_decoration_matches_reference() {
    check_all("end_", false);
}

/// Complete chain of a dimension (Rust terrain of the 3×3 then decoration by `DimGen`) against the
/// probe: separates a terrain difference from a decoration difference. `nether_` and `end_`.
fn pipeline_check(prefix: &str, settings: &str, dimension: &str) {
    let files = cases_with(prefix);
    if files.is_empty() {
        return;
    }
    let Some(w) = world() else { return };
    let mc = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft")).unwrap();
    let jar = mc.join("versions").join("26.3").join("26.3.jar");
    let v = rc_data::load_or_prepare("26.3", |m| eprintln!("rc-data : {m}")).expect("rc-data");
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    let wd = rc_worldgen::chunkgen::WorldData::load(&jar, Some(&reports), &v).unwrap();
    let names = if prefix == "nether_" { &w.nether_names } else { &w.end_names };
    let source_index: BTreeMap<String, u16> =
        names.iter().enumerate().map(|(i, n)| (rc_worldgen::data::full_id(n), i as u16)).collect();
    let spec = rc_worldgen::chunkgen::GenSpec::Noise {
        settings: settings.into(),
        source: if prefix == "nether_" {
            rc_worldgen::chunkgen::SourceSpec::MultiNoise(settings.into())
        } else {
            rc_worldgen::chunkgen::SourceSpec::TheEnd
        },
    };
    let (mut terrain_bad, mut deco_bad) = (0usize, 0usize);
    for path in &files {
        let case = read_case(path, &source_index);
        let dg = rc_worldgen::chunkgen::DimGen::new(&wd, &spec, dimension, case.seed).unwrap();
        let list: Vec<(i32, i32)> = (0..9).map(|k| (case.cx + k % 3 - 1, case.cz + k / 3 - 1)).collect();
        let mut cache = rc_worldgen::terrain::BiomeCache::default();
        dg.prepare_biomes(&mut cache, &list);
        let mut chunks: Vec<DecoChunk> = list.iter().map(|&(x, z)| dg.terrain(x, z, &cache, None)).collect();
        let mut tb = 0;
        let mut pairs: BTreeMap<(String, String), usize> = BTreeMap::new();
        let name = |s: u32| w.blocks.block_name(w.blocks.block(s)).to_owned();
        for (c, r) in chunks.iter().zip(&case.before) {
            for (a, b) in c.blocks.iter().zip(&r.blocks) {
                if a != b {
                    tb += 1;
                    *pairs.entry((name(*a), name(*b))).or_default() += 1;
                }
            }
        }
        {
            let mut it = chunks.iter_mut();
            let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
            dg.decorate(refs);
        }
        let mut db = 0;
        let mut dpairs: BTreeMap<(String, String), usize> = BTreeMap::new();
        for (k, c) in chunks.iter().enumerate() {
            for (a, b) in c.blocks.iter().zip(&case.after[k]) {
                if *a != u32::from(*b) {
                    db += 1;
                    *dpairs.entry((name(*a), name(u32::from(*b)))).or_default() += 1;
                }
            }
        }
        let top = |m: BTreeMap<(String, String), usize>| {
            let mut v: Vec<_> = m.into_iter().collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.1));
            v.truncate(6);
            v
        };
        eprintln!(
            "{}: terrain {tb} {:?}; decorated {db} {:?}",
            path.file_name().unwrap().to_string_lossy(),
            top(pairs),
            top(dpairs)
        );
        terrain_bad += tb;
        deco_bad += db;
    }
    assert_eq!((terrain_bad, deco_bad), (0, 0), "chain {prefix}");
}

#[test]
fn nether_pipeline_matches_reference() {
    pipeline_check("nether_", "minecraft:nether", "minecraft:the_nether");
}

#[test]
fn end_pipeline_matches_reference() {
    pipeline_check("end_", "minecraft:end", "minecraft:the_end");
}

#[test]
fn forced_features_match_reference() {
    check_all("force_", true);
}

/// Expected fingerprint of the forced geodes, taken on the old path
/// (`RUSTCRAFT_OPT_GEODE_REUSE=0 RUSTCRAFT_OPT_GEODE_ROWSKIP=0`).
const GEODE_FNV: u64 = 0x7b6e_a576_0aef_99b0;

/// Forced geodes (480 attempts of the placed feature on a game case): blocks identical to the old
/// path, stored fingerprint.
#[test]
fn forced_geodes_match_stored_digest() {
    let Some(path) = cases().into_iter().next() else { return };
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let source_index: BTreeMap<String, u16> =
        w.source_names.iter().enumerate().map(|(i, n)| (rc_worldgen::data::full_id(n), i as u16)).collect();
    let case = read_case(&path, &source_index);
    let deco = Decorator::new(&w.data, b, &w.source_names, case.seed).expect("decorator");
    let mut chunks = case.before;
    for c in &mut chunks {
        c.prime_final_heightmaps(b);
    }
    let before: Vec<Vec<u32>> = chunks.iter().map(|c| c.blocks.clone()).collect();
    {
        let mut reg = region(&mut chunks, b, case.seed, case.dim);
        deco.force_traced(&mut reg, "minecraft:amethyst_geode", 480, |_, _| {});
    }
    let changed: usize = chunks.iter().zip(&before).map(|(c, o)| c.blocks.iter().zip(o).filter(|(a, b)| a != b).count()).sum();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for c in &chunks {
        for &s in &c.blocks {
            for x in s.to_le_bytes() {
                h = (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    eprintln!("forced geodes: {changed} blocks changed, fingerprint {h:#018x}");
    assert!(changed > 1000, "too few blocks placed: {changed}");
    assert_eq!(h, GEODE_FNV, "fingerprint of the geodes");
}

/// Identifiers resolved once (`RUSTCRAFT_OPT_DECOR_IDS`) equal to the lookups by name.
#[test]
fn resolved_ids_match_name_lookups() {
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let deco = Decorator::new(&w.data, b, &w.source_names, 0).expect("decorator");
    assert_eq!(Some(b.k.dirt_path), b.block_id("dirt_path"));
    assert_eq!(Some(b.k.shelf_mushroom), b.block_id("shelf_mushroom"));
    for (got, name) in [
        (b.rules.beneath_bamboo, "beneath_bamboo_podzol_replaceable"),
        (b.rules.features_cannot_replace, "features_cannot_replace"),
        (b.rules.supports_chorus, "minecraft:supports_chorus_plant"),
    ] {
        assert_eq!(Some(got), b.tag(name), "{name}");
    }
    assert!(deco.pale_moss_patch.is_some());
    assert_eq!(deco.pale_moss_patch, deco.reg.feature_id("minecraft:pale_moss_patch"));
}

/// Freezing and snow: bound shortcut (`RUSTCRAFT_OPT_FREEZE_THRESHOLD`) and shared column
/// (`RUSTCRAFT_OPT_FREEZE_COL`) against the exact temperature, all biomes, mountains included.
#[test]
fn climate_shortcuts_match_exact() {
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let deco = Decorator::new(&w.data, b, &w.source_names, 0).expect("decorator");
    let sea = 63;
    // Every 3 heights: the thresholds of each biome fall between two.
    let ys: Vec<i32> = (-64..=320).step_by(3).collect();
    let (mut sure, mut cold_high, mut n) = (0usize, 0usize, 0usize);
    for biome in 0..w.source_names.len() as u16 {
        for gx in 0..30 {
            for gz in 0..30 {
                let (x, z) = (gx * 131 - 1900, gz * 117 - 1700);
                for &y in &ys {
                    let exact = |y: i32| deco.temperature(biome, [x, y, z], sea) >= 0.15;
                    let e = exact(y);
                    if deco.surely_warm(biome, y, sea) {
                        assert!(e, "biome {} ({x},{y},{z}): wrong bound", w.source_names[biome as usize]);
                        sure += 1;
                    }
                    if y > 80 && !e {
                        cold_high += 1;
                    }
                    assert_eq!(deco.warm_column(biome, x, z, [y - 1, y], sea), [exact(y - 1), e], "column ({x},{y},{z})");
                    n += 1;
                }
            }
        }
    }
    eprintln!("{n} cases; {sure} warm by the bound; {cold_high} cold above 80");
    assert!(sure > n / 4 && cold_high > 1000, "cases not varied enough: {sure} / {cold_high}");
}

/// Compares the draws of a feature with those of the probe (`DecorProbe rng:<feature>` produces the `.rng`).
/// `DECOR_CASE=<case> DECOR_RNG=minecraft:<feature> cargo test … --test decor_ref rng_trace -- --ignored --nocapture`
#[test]
#[ignore = "debugging"]
fn rng_trace() {
    let (Ok(case_f), Ok(name)) = (std::env::var("DECOR_CASE"), std::env::var("DECOR_RNG")) else { return };
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let source_index: BTreeMap<String, u16> =
        w.source_names.iter().enumerate().map(|(i, n)| (rc_worldgen::data::full_id(n), i as u16)).collect();
    let path = cases().into_iter().find(|p| p.to_string_lossy().contains(&case_f)).expect("case");
    let case = read_case(&path, &source_index);
    let deco = Decorator::new(&w.data, b, &w.source_names, case.seed).unwrap();
    let mut chunks = case.before;
    for c in &mut chunks {
        c.prime_final_heightmaps(b);
    }
    let before: Vec<Vec<u32>> = chunks.iter().map(|c| c.blocks.clone()).collect();
    let mut reg = region(&mut chunks, b, case.seed, case.dim);
    let (ours, sets) = deco.decorate_rng_trace(&mut reg, &name);
    let java_sets: Vec<([i32; 3], u32)> = std::fs::read_to_string(path.with_extension("rng"))
        .unwrap()
        .lines()
        .filter(|l| l.starts_with("S "))
        .map(|l| {
            let v: Vec<i64> = l[2..].split(' ').map(|x| x.parse().unwrap()).collect();
            ([v[0] as i32, v[1] as i32, v[2] as i32], v[3] as u32)
        })
        .collect();
    eprintln!("placements: us {}, game {}", sets.len(), java_sets.len());
    if let Some(i) = (0..sets.len().min(java_sets.len())).find(|&i| sets[i] != java_sets[i]) {
        eprintln!("first different placement {i}");
        for k in i.saturating_sub(3)..(i + 5).min(sets.len()).min(java_sets.len()) {
            eprintln!(
                "  {k}: us {:?} {} / game {:?} {}",
                sets[k].0,
                b.describe(sets[k].1),
                java_sets[k].0,
                b.describe(java_sets[k].1)
            );
        }
    }
    if let Ok(at) = std::env::var("DECOR_AT") {
        for p in at.split(';') {
            let v: Vec<i32> = p.split(',').map(|x| x.parse().unwrap()).collect();
            let k = (((v[2] >> 4) - case.cz + 1) * 3 + ((v[0] >> 4) - case.cx + 1)) as usize;
            let i = ((((v[1] - MIN_Y) << 4) | (v[2] & 15)) << 4 | (v[0] & 15)) as usize;
            eprintln!(
                "{p}: before {} / game end {} / us end {}",
                b.describe(before[k][i]),
                b.describe(u32::from(case.after[k][i])),
                b.describe(reg.chunks[k].blocks[i])
            );
        }
    }
    let theirs: Vec<(u32, i32, String)> = std::fs::read_to_string(path.with_extension("rng"))
        .unwrap()
        .lines()
        .filter(|l| !l.starts_with("F ") && !l.starts_with("S "))
        .map(|l| {
            let mut it = l.splitn(3, ' ');
            (
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap().parse().unwrap(),
                it.next().unwrap_or("").to_owned(),
            )
        })
        .collect();
    eprintln!("draws: us {}, game {}", ours.len(), theirs.len());
    // Labels: 1 leaf skip, 2 hanging extension, 0 other.
    let cat = |c: &str| {
        if c.contains("shouldSkipLocation") {
            1
        } else if c.contains("tryPlaceExtension") {
            2
        } else {
            0
        }
    };
    for (i, t) in theirs.iter().enumerate() {
        let Some(o) = ours.get(i) else { break };
        let (oc, tc) = (o.0 / 100, cat(&t.2));
        if (oc == 1 || oc == 2 || tc == 1 || tc == 2) && oc != tc {
            eprintln!("category divergence at draw {i}: us {oc}, game {tc} ({})", t.2);
            for k in i.saturating_sub(5)..(i + 3).min(theirs.len()) {
                eprintln!("  {k}: us {:?} / game {} {}", ours.get(k), theirs[k].0, theirs[k].2);
            }
            break;
        }
    }
    let ours: Vec<(u32, i32)> = ours.into_iter().map(|(b, v)| (b % 100, v)).collect();
    for (i, t) in theirs.iter().enumerate() {
        match ours.get(i) {
            Some(o) if o.0 == t.0 && o.1 == t.1 => {}
            o => {
                eprintln!("divergence at draw {i}: us {o:?}, game ({}, {}) {}", t.0, t.1, t.2);
                for k in i.saturating_sub(6)..i {
                    eprintln!("  before {k}: {:?} / {}", ours.get(k), theirs[k].2);
                }
                for k in i..(i + 4).min(theirs.len()) {
                    eprintln!(
                        "  after {k}: us {:?} / game {} {} {}",
                        ours.get(k),
                        theirs[k].0,
                        theirs[k].1,
                        theirs[k].2
                    );
                }
                return;
            }
        }
    }
    eprintln!("identical over {} draws", theirs.len());
}

/// Compares Rust's sin, cos, pow, ln, sqrt with those of Java (`TRIG_REF` = probe file).
#[test]
#[ignore = "debugging"]
fn trig_matches_java() {
    let Ok(p) = std::env::var("TRIG_REF") else { return };
    let text = std::fs::read_to_string(p).unwrap();
    let mut bad = [0usize; 5];
    let mut jbad = [0usize; 2];
    let mut n = 0;
    for l in text.lines() {
        let v: Vec<u64> = l.split(' ').map(|x| x.parse::<i64>().unwrap() as u64).collect();
        let x = f64::from_bits(v[0]);
        let p = f64::from_bits(v[6]);
        let ours = [x.sin(), x.cos(), p.powf(1.333_333_333_333_333_3), p.ln(), p.sqrt()];
        for k in 0..5 {
            if ours[k].to_bits() != v[k + 1] {
                bad[k] += 1;
            }
        }
        // Rounding to nearest (rc_worldgen::jmath), used by generation.
        for (k, j) in [rc_worldgen::jmath::sin(x), rc_worldgen::jmath::cos(x)].into_iter().enumerate() {
            if j.to_bits() != v[k + 1] {
                jbad[k] += 1;
            }
        }
        n += 1;
    }
    eprintln!("{n} values; differences sin {} cos {} pow {} ln {} sqrt {}", bad[0], bad[1], bad[2], bad[3], bad[4]);
    eprintln!("rounded to nearest: differences sin {} cos {}", jbad[0], jbad[1]);
    assert!(jbad[0] * 200 < n && jbad[1] * 200 < n, "rounded sin/cos: more than 0.5% differences from the game");
}

/// Time per top-level feature, accumulated over all the cases (best of 3).
#[test]
#[ignore = "benchmark"]
fn decoration_profile() {
    let files = cases();
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let source_index: BTreeMap<String, u16> =
        w.source_names.iter().enumerate().map(|(i, n)| (rc_worldgen::data::full_id(n), i as u16)).collect();
    let mut decos: BTreeMap<i64, Decorator> = BTreeMap::new();
    let mut per: BTreeMap<String, f64> = BTreeMap::new();
    for path in &files {
        let case = read_case(path, &source_index);
        let deco =
            decos.entry(case.seed).or_insert_with(|| Decorator::new(&w.data, b, &w.source_names, case.seed).unwrap());
        let mut best: BTreeMap<String, f64> = BTreeMap::new();
        for _ in 0..3 {
            let mut chunks: Vec<DecoChunk> = case.before.clone();
            for c in &mut chunks {
                c.prime_final_heightmaps(b);
            }
            let mut reg = region(&mut chunks, b, case.seed, case.dim);
            let mut t = Instant::now();
            let mut this: BTreeMap<String, f64> = BTreeMap::new();
            deco.decorate_traced(&mut reg, |_, _, f| {
                *this.entry(deco.reg.placed_name(f).to_owned()).or_insert(0.0) += t.elapsed().as_secs_f64();
                t = Instant::now();
            });
            for (k, v) in this {
                let e = best.entry(k).or_insert(f64::MAX);
                *e = e.min(v);
            }
        }
        for (k, v) in best {
            *per.entry(k).or_insert(0.0) += v;
        }
    }
    let mut v: Vec<(String, f64)> = per.into_iter().collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = v.iter().map(|x| x.1).sum();
    eprintln!(
        "{} cases, {:.1} ms in total ({:.2} ms / chunk)",
        files.len(),
        total * 1e3,
        total * 1e3 / files.len() as f64
    );
    for (k, t) in v.iter().take(25) {
        eprintln!("  {:6.2} ms  {k}", t * 1e3);
    }
}

#[test]
#[ignore = "benchmark"]
fn decoration_timing() {
    let files = cases();
    let Some(w) = world() else { return };
    let b = &w.blocks;
    let source_index: BTreeMap<String, u16> =
        w.source_names.iter().enumerate().map(|(i, n)| (rc_worldgen::data::full_id(n), i as u16)).collect();
    let t = Instant::now();
    let mut decos: BTreeMap<i64, Decorator> = BTreeMap::new();
    for path in &files {
        let case = read_case(path, &source_index);
        let deco =
            decos.entry(case.seed).or_insert_with(|| Decorator::new(&w.data, b, &w.source_names, case.seed).unwrap());
        let mut best = f64::MAX;
        for _ in 0..5 {
            let mut chunks: Vec<DecoChunk> = case.before.clone();
            for c in &mut chunks {
                c.prime_final_heightmaps(b);
            }
            let mut reg = region(&mut chunks, b, case.seed, case.dim);
            let t0 = Instant::now();
            deco.decorate(&mut reg);
            best = best.min(t0.elapsed().as_secs_f64());
        }
        eprintln!("{} : {:.2} ms", path.file_name().unwrap().to_string_lossy(), best * 1e3);
    }
    eprintln!("total {:.1} s", t.elapsed().as_secs_f64());
}
