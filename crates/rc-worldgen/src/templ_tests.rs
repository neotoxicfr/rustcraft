//! Equivalence proofs of the templates, of the jigsaw assembly and of the placement (C1-M11). Each
//! path is forced on the current thread (`opt::force`); game data absent: tests skipped,
//! except `RUSTCRAFT_OPT_REQUIRE=1` (explicit failure, see `structure_tests`).

use std::fmt::Write as _;
use std::sync::Arc;

use super::tests::{dim, world_data};
use crate::chunkgen::{DimGen, GenSpec, SourceSpec, WorldData};
use crate::decor_region::DecoChunk;
use crate::jigsaw::Element;
use crate::opt;
use crate::structure::{Kind, Piece, Start};
use crate::template::{StateTransforms, Templates};

const ALL: [&str; 10] = opt::C1M11;
const SEEDS: [i64; 3] = [20261011, 1, -4172144997902289642];

/// Forced path: all old, all new, or a single new track.
#[derive(Debug, Clone, Copy)]
enum Mode {
    Old,
    New,
    Only(&'static str),
}

fn set(m: Mode) {
    match m {
        Mode::Old => opt::force(&ALL, Some(false)),
        Mode::New => opt::force(&ALL, Some(true)),
        Mode::Only(n) => {
            opt::force(&ALL, Some(false));
            opt::force(&[n], Some(true));
        }
    }
}

fn modes() -> Vec<Mode> {
    let mut v = vec![Mode::Old, Mode::New];
    v.extend(ALL.iter().map(|n| Mode::Only(n)));
    v
}

/// FNV-1a fingerprint written along a `Debug` (without intermediate string).
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl std::fmt::Write for Fnv {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        for b in s.bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        Ok(())
    }
}

fn nether(w: &WorldData, seed: i64) -> DimGen {
    let spec = GenSpec::Noise {
        settings: "minecraft:nether".into(),
        source: SourceSpec::MultiNoise("minecraft:nether".into()),
    };
    let mut dg = DimGen::new(w, &spec, "minecraft:the_nether", seed).unwrap();
    dg.enable_structures(w).unwrap();
    dg
}

fn elem_name(e: &Element) -> String {
    match e {
        Element::Single { location, .. } => location.clone(),
        Element::List { elements, .. } => {
            format!("L({})", elements.iter().map(|e| elem_name(e)).collect::<Vec<_>>().join(","))
        }
        Element::Feature { feature, .. } => format!("F{feature}"),
        Element::Empty => "-".into(),
    }
}

/// Complete form of a list of starts: pieces (element, position, rotation, box,
/// junctions, ground, water).
fn shape(st: &[Start]) -> String {
    let mut s = String::new();
    for x in st {
        let _ = write!(s, "[{} {},{} {:?}", x.structure, x.cx, x.cz, x.bbox);
        for p in &x.pieces {
            match p {
                Piece::Jigsaw(j) => {
                    let _ = write!(
                        s,
                        " J{} {:?} {} {:?} {:?} {:?} {}",
                        elem_name(&j.element),
                        j.pos,
                        j.ground_delta,
                        j.rot,
                        j.bbox,
                        j.junctions,
                        j.waterlog
                    );
                }
                Piece::Coded { id, bbox, .. } => {
                    let _ = write!(s, " C{id} {bbox:?}");
                }
            }
        }
        s += "]";
    }
    s
}

/// Jigsaw starts: each path (all old, all new, each track alone) gives the
/// same pieces as the old code, chunk by chunk, on 3 seeds, main world and Nether.
#[test]
fn jigsaw_starts_every_path_matches_old() {
    let Some(w) = world_data() else { return };
    let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
    let mut bad: Vec<String> = Vec::new();
    for seed in SEEDS {
        for dg in [dim(w, Some(seed)), nether(w, seed)] {
            let sg = dg.structures.clone().unwrap();
            let t = dg.structure_terrain().unwrap();
            let r = 128;
            set(Mode::Old);
            let mut reference = Vec::new();
            for cx in -r..r {
                for cz in -r..r {
                    let st = sg.compute_starts(cx, cz, t, &dg.blocks, true);
                    for s in &st {
                        if matches!(sg.defs[s.structure].kind, Kind::Jigsaw(_)) {
                            *counts.entry(sg.defs[s.structure].id.clone()).or_default() += 1;
                        }
                    }
                    reference.push(shape(&st));
                }
            }
            for m in modes().into_iter().skip(1) {
                set(m);
                // All new on the whole square, each track alone on the central square.
                let rr = if matches!(m, Mode::New) { r } else { 64 };
                for cx in -rr..rr {
                    for cz in -rr..rr {
                        let i = ((cx + r) * 2 * r + (cz + r)) as usize;
                        let got = shape(&sg.compute_starts(cx, cz, t, &dg.blocks, true));
                        if got != reference[i] && !bad.iter().any(|b: &String| b.starts_with(&format!("{m:?}"))) {
                            bad.push(format!("{m:?} seed {seed} chunk ({cx}, {cz})"));
                        }
                    }
                }
            }
        }
    }
    opt::force(&ALL, None);
    eprintln!("jigsaw starts compared: {counts:?}");
    assert!(bad.is_empty(), "paths different from the old one: {bad:?}");
    let total: usize = counts.values().sum();
    assert!(total >= 200, "too few jigsaw starts ({total}): pointless test");
}

/// Template table: 32 threads all request all the templates (shifted orders): a single decoding
/// per template, a single `Arc` per template; absent identifier = `None` without decoding; content =
/// old path, and fingerprint of all the templates = stored value.
#[test]
fn templates_table_one_decode_per_template() {
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let new = Templates::new(w.decor.files_of("template"), b);
    let mut ids: Vec<String> = new.ids().map(String::from).collect();
    ids.sort();
    assert!(ids.len() > 500, "templates missing: {}", ids.len());
    let got: Vec<Vec<usize>> = std::thread::scope(|s| {
        let hs: Vec<_> = (0..32)
            .map(|k| {
                let (ids, new) = (&ids, &new);
                s.spawn(move || {
                    opt::force(&["TEMPL_TABLE"], Some(true));
                    let n = ids.len();
                    let mut ptr = vec![0usize; n];
                    for i in 0..n {
                        let j = (i + k * 37) % n;
                        // Short form (without namespace) for half of the threads.
                        let id = if k % 2 == 0 { ids[j].as_str() } else { ids[j].trim_start_matches("minecraft:") };
                        ptr[j] = new.get(id, b).map_or(0, |t| Arc::as_ptr(&t) as usize);
                    }
                    ptr
                })
            })
            .collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(new.decode_count(), ids.len(), "duplicate or missing decodings");
    for g in &got[1..] {
        assert_eq!(g, &got[0], "two distinct templates for the same identifier");
    }
    opt::force(&["TEMPL_TABLE"], Some(true));
    assert!(new.get("minecraft:never/seen", b).is_none());
    assert_eq!(new.decode_count(), ids.len(), "an absent identifier was decoded");
    let old = Templates::new(w.decor.files_of("template"), b);
    let mut all = Fnv::new();
    for id in &ids {
        opt::force(&["TEMPL_TABLE"], Some(false));
        let a = old.get(id, b);
        opt::force(&["TEMPL_TABLE"], Some(true));
        let n = new.get(id, b);
        let (mut ha, mut hn) = (Fnv::new(), Fnv::new());
        content(&mut ha, &a);
        content(&mut hn, &n);
        assert_eq!(ha.0, hn.0, "template {id} differs");
        let _ = write!(all, "{id}");
        content(&mut all, &a);
    }
    opt::force(&ALL, None);
    eprintln!("template fingerprint: 0x{:016x} ({} templates)", all.0, ids.len());
    assert_eq!((all.0, ids.len()), STORED_TEMPLATES, "templates differ from the stored value");
}

/// Content of a template as the old code kept it (without the added category ends).
fn content(h: &mut Fnv, t: &Option<Arc<crate::template::Template>>) {
    let _ = match t {
        Some(t) => write!(h, "{:?}{:?}{:?}{:?}", t.size, t.palettes, t.jigsaws, t.entities),
        None => write!(h, "-"),
    };
}

/// Fingerprint of all the decoded templates (old code, 26.3) and their number.
const STORED_TEMPLATES: (u64, usize) = (0x6368_39ae_8c5e_42c3, 1511);

/// State transformation tables: fast path = old, element by element, and stored
/// value.
#[test]
fn state_transforms_fast_matches_old() {
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    // Three alternating passes, best of each.
    let (mut to, mut tf) = (f64::MAX, f64::MAX);
    for _ in 0..3 {
        let t0 = std::time::Instant::now();
        drop(StateTransforms::new_mode(b, false));
        let t1 = std::time::Instant::now();
        drop(StateTransforms::new_mode(b, true));
        to = to.min((t1 - t0).as_secs_f64() * 1e3);
        tf = tf.min(t1.elapsed().as_secs_f64() * 1e3);
    }
    eprintln!("state transformations: old {to:.1} ms, fast {tf:.1} ms (loaded machine)");
    let old = StateTransforms::new_mode(b, false);
    let fast = StateTransforms::new_mode(b, true);
    eprintln!("fingerprint: 0x{:016x}", old.digest());
    assert_eq!(fast.digest(), old.digest(), "different tables");
    assert_eq!(old.digest(), STORED_TRANSFORMS, "tables differ from the stored value");
}

/// Fingerprint of the tables rot[1..3] then mirror[0..1] (old code, 26.3).
const STORED_TRANSFORMS: u64 = 0x4edf_96dd_82e9_8a0b;

/// Fingerprint of a decorated area: states, block entity data (loot seeds included),
/// entities, ticks and post-processing marks of the 9 chunks.
fn area_digest(chunks: &[DecoChunk]) -> u64 {
    let mut h = Fnv::new();
    for c in chunks {
        let _ = write!(
            h,
            "{},{} {:?} {:?} {:?} {:?} {:?} {:?}",
            c.cx, c.cz, c.blocks, c.be_data, c.entities, c.fluid_ticks, c.block_ticks, c.post_process
        );
    }
    h.0
}

/// Chunks to decorate: for the first starts of each structure with templates, the chunk
/// of the centre of the first piece and that of the low corner of the box (straddling piece).
fn placement_cases(dg: &DimGen, r: i32, per: usize) -> Vec<(String, i32, i32)> {
    let sg = dg.structures.clone().unwrap();
    let t = dg.structure_terrain().unwrap();
    let mut seen: hashbrown::HashMap<usize, usize> = hashbrown::HashMap::new();
    let mut out = Vec::new();
    for cx in -r..r {
        for cz in -r..r {
            for s in sg.starts(cx, cz, t, &dg.blocks).iter() {
                let templated = matches!(
                    sg.defs[s.structure].kind,
                    Kind::Jigsaw(_)
                        | Kind::RuinedPortal(_)
                        | Kind::Shipwreck { .. }
                        | Kind::OceanRuin { .. }
                        | Kind::Igloo
                        | Kind::NetherFossil(_)
                        | Kind::EndCity
                );
                let n = seen.entry(s.structure).or_default();
                if !templated || *n >= per {
                    continue;
                }
                *n += 1;
                let Some(first) = s.pieces.first() else { continue };
                let c = first.bbox().center();
                let id = sg.defs[s.structure].id.clone();
                out.push((id.clone(), c[0] >> 4, c[2] >> 4));
                out.push((id, s.bbox.min[0] >> 4, s.bbox.min[2] >> 4));
            }
        }
    }
    out
}

fn decorated(dg: &DimGen, base: &[DecoChunk]) -> u64 {
    let mut v = base.to_vec();
    let arr: [&mut DecoChunk; 9] = v.iter_mut().collect::<Vec<_>>().try_into().unwrap();
    dg.decorate(arr);
    area_digest(&v)
}

fn terrain3(dg: &DimGen, cx: i32, cz: i32) -> Vec<DecoChunk> {
    let targets: Vec<(i32, i32)> = (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (cx + dx, cz + dz))).collect();
    let mut cache = crate::terrain::BiomeCache::default();
    dg.prepare_biomes(&mut cache, &targets);
    targets.iter().map(|&(x, z)| dg.terrain(x, z, &cache, None)).collect()
}

/// Placement: complete decoration (structures included) of the 3×3 chunks around pieces of
/// villages, bastions, cities, shipwrecks, ruins, portals, fossils… = old path, for each
/// path (all new, each track alone).
#[test]
fn placement_every_path_matches_old() {
    let Some(w) = world_data() else { return };
    let mut n = 0;
    let mut bad: Vec<String> = Vec::new();
    for dg in [dim(w, Some(20261011)), nether(w, 20261011), dim(w, Some(-4172144997902289642))] {
        for (id, cx, cz) in placement_cases(&dg, 64, 4) {
            let base = terrain3(&dg, cx, cz);
            set(Mode::Old);
            // A first placement fixes the adjustments kept in the pieces (height of the shipwrecks,
            // draw included): the reference is the second, like all the following ones.
            decorated(&dg, &base);
            let reference = decorated(&dg, &base);
            for m in modes().into_iter().skip(1) {
                set(m);
                if decorated(&dg, &base) != reference && !bad.iter().any(|b: &String| b.starts_with(&format!("{m:?} "))) {
                    bad.push(format!("{m:?} {id} at ({cx}, {cz})"));
                }
            }
            n += 1;
        }
    }
    opt::force(&ALL, None);
    eprintln!("{n} decorated areas compared");
    assert!(bad.is_empty(), "paths different from the old one: {bad:?}");
    assert!(n >= 20, "too few areas ({n}): pointless test");
}

/// Sources placed by the template: they do not waterlog a neighbour of the same template (the two
/// forms of the locked sources list), whereas a source already there does.
#[test]
fn locked_sources_do_not_waterlog() {
    use crate::decor_region::Region;
    use crate::structure_place::{Placing, place_template};
    use crate::template::{TBlock, Template};
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let (water, stairs) = (b.state_from_str("minecraft:water").unwrap(), b.state_from_str("minecraft:oak_stairs").unwrap());
    let wl = b.prop("waterlogged").unwrap();
    let stairs = b.with_bool(stairs, wl, false);
    let t = Template {
        size: [2, 1, 1],
        palettes: vec![vec![
            TBlock { pos: [0, 0, 0], state: water, nbt: None },
            TBlock { pos: [1, 0, 0], state: stairs, nbt: None },
        ]],
        cats: vec![[0, 2]],
        jigsaws: vec![Vec::new()],
        entities: Vec::new(),
    };
    let s = Placing {
        mirror: crate::template::Mirror::None,
        rot: crate::template::Rot::None,
        pivot: [0, 0, 0],
        bbox: None,
        procs: Vec::new().into(),
        waterlog: true,
        seed: None,
        shared: false,
        known_shape: true,
        entities: None,
    };
    for (set_on, pre_water) in [(false, false), (true, false), (false, true), (true, true)] {
        opt::force(&["LOCKED_SET"], Some(set_on));
        let mut chunks: Vec<DecoChunk> =
            (-1..=1).flat_map(|z| (-1..=1).map(move |x| DecoChunk::new(x, z, -64, 384))).collect();
        let arr: [&mut DecoChunk; 9] = chunks.iter_mut().collect::<Vec<_>>().try_into().unwrap();
        let mut r = Region::new(arr, b, 63, 1);
        if pre_water {
            // Source already above the staircase: this one is not locked.
            r.set_p([9, 71, 8], water, 3);
        }
        let mut rng = crate::decor_rng::DecoRng::legacy(1);
        assert!(place_template(&mut r, b, &w.templates.transforms, 1, &t, [8, 70, 8], [8, 70, 8], &s, &mut rng, 2));
        let got = b.get_bool(r.get_p([9, 70, 8]), wl);
        assert_eq!(got, Some(pre_water), "set {set_on}, source above {pre_water}");
    }
    opt::force(&ALL, None);
}

/// Local box of the placement = exact antecedent of the area (all mirrors, rotations, pivots).
#[test]
fn local_box_is_exact_preimage() {
    use crate::structure_place::{Placing, local_box};
    use crate::template::{BBox, Mirror, Rot, transform};
    let mut rng = crate::decor_rng::DecoRng::legacy(1);
    let mut n = |k: i32| rng.next_int_bounded(2 * k + 1) - k;
    for _ in 0..1500 {
        let mirror = [Mirror::None, Mirror::LeftRight, Mirror::FrontBack][(n(1) + 1) as usize];
        let rot = Rot::from_index(n(2));
        let pivot = [n(4), 0, n(4)];
        let pos = [n(20), n(4), n(20)];
        let bb = BBox::new([n(16), n(6), n(16)], [n(16), n(6), n(16)]);
        let s = Placing {
            mirror,
            rot,
            pivot,
            bbox: Some(bb),
            procs: Vec::new().into(),
            waterlog: false,
            seed: None,
            shared: false,
            known_shape: true,
            entities: None,
        };
        let l = local_box(&bb, pos, &s);
        for x in -14..14 {
            for y in -8..8 {
                for z in -14..14 {
                    let p = transform([x, y, z], mirror, rot, pivot);
                    let w = [p[0] + pos[0], p[1] + pos[1], p[2] + pos[2]];
                    assert_eq!(bb.contains(w), l.contains([x, y, z]), "{mirror:?} {rot:?} {pivot:?} {pos:?} {bb:?}");
                }
            }
        }
    }
}

/// Micro-bench (loaded machine): starts of a 128 × 128 chunk square, hot (templates
/// and caches already filled by one pass of each path), 3 alternating passes (best kept)
/// of the old path, the new one and each assembly track alone, at 1 thread then by
/// powers of 2 up to the number of logical threads.
/// `cargo test --release -p rc-worldgen bench_jigsaw_starts -- --ignored --nocapture`
#[test]
#[ignore = "benchmark"]
fn bench_jigsaw_starts() {
    use rayon::prelude::*;
    let Some(w) = world_data() else { return };
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut nts = vec![1];
    while nts.last().unwrap() * 2 < threads {
        nts.push(nts.last().unwrap() * 2);
    }
    nts.push(threads);
    let list: Vec<(i32, i32)> = (-64..64).flat_map(|x| (-64..64).map(move |z| (x, z))).collect();
    let ms = [
        Mode::Old,
        Mode::New,
        Mode::Only("TEMPL_TABLE"),
        Mode::Only("ELEM_ONCE"),
        Mode::Only("POOLS_TABLE"),
        Mode::Only("JIGSAW_PRECALC"),
        Mode::Only("JIGSAW_IDX"),
    ];
    for (name, dg) in [("world", dim(w, Some(20261011))), ("nether", nether(w, 20261011))] {
        let sg = dg.structures.clone().unwrap();
        let t = dg.structure_terrain().unwrap();
        let pools: Vec<rayon::ThreadPool> =
            nts.iter().map(|&n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap()).collect();
        let run = |m: Mode, pool: &rayon::ThreadPool| {
            let t0 = std::time::Instant::now();
            let n: usize = pool.install(|| {
                list.par_iter()
                    .map(|&(x, z)| {
                        set(m);
                        sg.compute_starts(x, z, t, &dg.blocks, true).len()
                    })
                    .sum()
            });
            (t0.elapsed().as_secs_f64() * 1e3, n)
        };
        let last = pools.last().unwrap();
        run(Mode::Old, last);
        run(Mode::New, last);
        let mut best = vec![vec![f64::MAX; nts.len()]; ms.len()];
        let mut count = 0;
        for _ in 0..3 {
            for (k, p) in pools.iter().enumerate() {
                for (i, &m) in ms.iter().enumerate() {
                    let (dt, n) = run(m, p);
                    count = n;
                    best[i][k] = best[i][k].min(dt);
                }
            }
        }
        for (i, m) in ms.iter().enumerate() {
            let row: Vec<String> = nts.iter().zip(&best[i]).map(|(n, d)| format!("{n} threads {d:.0} ms")).collect();
            eprintln!("{name} {m:?} ({count} starts): {}", row.join(", "));
        }
    }
    opt::force(&ALL, None);
}


/// Micro-bench (loaded machine): decoration of the areas of [`placement_every_path_matches_old`], old
/// path, all new, each track alone (1 thread: the placement of an area is sequential).
#[test]
#[ignore = "benchmark"]
fn bench_placement() {
    let Some(w) = world_data() else { return };
    let mut zones = Vec::new();
    for dg in [dim(w, Some(20261011)), nether(w, 20261011)] {
        let cases = placement_cases(&dg, 48, 2);
        let bases: Vec<Vec<DecoChunk>> = cases.iter().map(|&(_, x, z)| terrain3(&dg, x, z)).collect();
        zones.push((dg, bases));
    }
    for m in modes() {
        set(m);
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t0 = std::time::Instant::now();
            for (dg, bases) in &zones {
                for b in bases {
                    decorated(dg, b);
                }
            }
            best = best.min(t0.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!("{m:?}: {best:.0} ms (best of 3)");
    }
    opt::force(&ALL, None);
}

/// Micro-bench (loaded machine): placement of the 24 largest templates, each straddling the
/// central chunk (area = this chunk), processors of a degradation list, 3 alternating passes
/// per path.
#[test]
#[ignore = "benchmark"]
fn bench_place_template() {
    use crate::decor_region::Region;
    use crate::structure_place::{Placing, ProcChain, ProcLists, place_template};
    use crate::template::{BBox, Mirror, Rot};
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    opt::force(&ALL, Some(true));
    let mut big: Vec<(usize, Arc<crate::template::Template>)> = w
        .templates
        .ids()
        .filter_map(|id| w.templates.get(id, b))
        .map(|t| (t.palettes.first().map_or(0, Vec::len), t))
        .collect();
    big.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    big.truncate(24);
    let lists = ProcLists::new(&w.decor);
    let mut ids = w.decor.ids("processor_list");
    ids.sort();
    let pid = ids.iter().find(|i| i.contains("degradation")).cloned().unwrap_or_default();
    let procs = lists.get(&crate::json::Json::Str(pid.clone()), b);
    let base: Vec<DecoChunk> = (-1..=1).flat_map(|z| (-1..=1).map(move |x| DecoChunk::new(x, z, -64, 384))).collect();
    let zone = BBox::new([0, -63, 0], [15, 319, 15]);
    let total: usize = big.iter().map(|(n, _)| n).sum();
    for _ in 0..2 {
        for m in modes() {
            set(m);
            let mut best = f64::MAX;
            for _ in 0..3 {
                let mut chunks = base.clone();
                let t0 = std::time::Instant::now();
                for (k, (_, t)) in big.iter().enumerate() {
                    let arr: [&mut DecoChunk; 9] = chunks.iter_mut().collect::<Vec<_>>().try_into().unwrap();
                    let mut r = Region::new(arr, b, 63, 1);
                    let s = Placing {
                        mirror: Mirror::None,
                        rot: Rot::from_index(k as i32),
                        pivot: [0, 0, 0],
                        bbox: Some(zone),
                        procs: ProcChain::of(procs.clone()),
                        waterlog: true,
                        seed: None,
                        shared: false,
                        known_shape: true,
                        entities: None,
                    };
                    let mut rng = crate::decor_rng::DecoRng::legacy(k as i64);
                    // Template corner 8 blocks before the chunk: it crosses it.
                    place_template(&mut r, b, &w.templates.transforms, 1, t, [-8, 0, -8], [0, 0, 0], &s, &mut rng, 2);
                }
                best = best.min(t0.elapsed().as_secs_f64() * 1e3);
            }
            eprintln!("{m:?}: {best:.2} ms ({} templates, {total} blocks, list {pid})", big.len());
        }
    }
    opt::force(&ALL, None);
}
