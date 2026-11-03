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
use crate::decor_blocks::State;
use crate::opt::TablesThread;
use crate::template::{StateTransforms, TableBuild, Templates, TransformTables};

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

/// Every build of the state tables: each lead alone and every combination (16).
fn every_table_build() -> Vec<TableBuild> {
    (0..16u8)
        .map(|m| TableBuild {
            skip_unoriented: m & 1 != 0,
            compose_rotations: m & 2 != 0,
            per_block: m & 4 != 0,
            parallel: m & 8 != 0,
        })
        .collect()
}

const TABLE_NAMES: [&str; 5] = ["quarter turn", "half turn", "three-quarter turn", "left-right mirror", "front-back mirror"];

/// State transformation tables: every build (each lead alone and every combination) gives the
/// tables of the old build, state by state, and the old build gives the stored value.
#[test]
fn every_table_build_matches_old_tables() {
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let old = StateTransforms::build(b, TableBuild::default());
    eprintln!("fingerprint: 0x{:016x}", old.digest());
    assert_eq!(old.digest(), STORED_TRANSFORMS, "tables differ from the stored value");
    for how in every_table_build() {
        let new = StateTransforms::build(b, how);
        for (k, (o, n)) in old.tables().into_iter().zip(new.tables()).enumerate() {
            assert_eq!(o.len(), n.len(), "{how:?}: length of the {} table", TABLE_NAMES[k]);
            if let Some(s) = o.iter().zip(n).position(|(a, c)| a != c) {
                panic!(
                    "{how:?}: {} of {} gives {} instead of {}",
                    TABLE_NAMES[k],
                    b.describe(s as State),
                    b.describe(n[s]),
                    b.describe(o[s])
                );
            }
        }
    }
}

/// Tables built off the critical path: inline, on a dedicated thread and as a rayon task, they
/// are the stored tables; 16 threads that need them at once share a single build; tables kept
/// under a key are handed, already built, to the next world with that key.
#[test]
fn state_tables_off_the_critical_path() {
    let Some(w) = world_data() else { return };
    for thread in [TablesThread::Inline, TablesThread::Dedicated, TablesThread::RayonPool] {
        let tables = TransformTables::start(&w.blocks, None, thread);
        let seen: Vec<usize> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..16).map(|_| s.spawn(|| &*tables as *const StateTransforms as usize)).collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(seen.iter().all(|&p| p == seen[0]), "{thread:?}: two builds of the tables");
        assert_eq!(tables.digest(), STORED_TRANSFORMS, "{thread:?}: tables differ from the stored value");
    }
    let key = || Some("test|state tables kept".to_owned());
    let first = TransformTables::start(&w.blocks, key(), TablesThread::Dedicated);
    let first_ptr = &*first as *const StateTransforms;
    let again = TransformTables::start(&w.blocks, key(), TablesThread::Dedicated);
    assert!(again.is_built(), "kept tables rebuilt");
    assert!(std::ptr::eq(first_ptr, &*again), "kept tables not shared");
    assert!(TransformTables::start(&w.blocks, None, TablesThread::Inline).is_built(), "inline tables not built");
}

/// Bench (loaded machine, `--ignored`): the five state tables by lead and combination on one
/// thread, the parallel build from 1 thread to all, and the tables built inline or in the
/// background while the stronghold rings occupy the rayon pool (as at generator start).
#[test]
#[ignore = "bench"]
fn bench_state_transforms() {
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
    let mut procs = f64::MAX;
    for _ in 0..10 {
        let t = std::time::Instant::now();
        std::hint::black_box(crate::structure_procs::ProcTables::new(b));
        procs = procs.min(ms(t));
    }
    eprintln!("processor tables alone: {procs:.2} ms");
    let sequential: Vec<TableBuild> = every_table_build().into_iter().filter(|h| !h.parallel).collect();
    let mut best = vec![f64::MAX; sequential.len()];
    for _ in 0..5 {
        for (k, how) in sequential.iter().enumerate() {
            let t = std::time::Instant::now();
            std::hint::black_box(StateTransforms::build(b, *how));
            best[k] = best[k].min(ms(t));
        }
    }
    for (how, t) in sequential.iter().zip(&best) {
        eprintln!("1 thread, {how:?}: {t:.2} ms");
    }
    let all = std::thread::available_parallelism().map_or(1, usize::from);
    let fastest = TableBuild { skip_unoriented: true, compose_rotations: true, per_block: true, parallel: true };
    let mut threads = 1;
    while threads <= all {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        let mut t_best = f64::MAX;
        for _ in 0..10 {
            let t = std::time::Instant::now();
            pool.install(|| std::hint::black_box(StateTransforms::build(b, fastest)));
            t_best = t_best.min(ms(t));
        }
        eprintln!("parallel build, {threads} threads: {t_best:.2} ms");
        threads = if threads == all { all + 1 } else { (threads * 2).min(all) };
    }
    for _ in 0..2 {
        for thread in [TablesThread::Inline, TablesThread::Dedicated, TablesThread::RayonPool] {
            let dg = dim(w, Some(SEEDS[0]));
            let t = std::time::Instant::now();
            let tables = TransformTables::start(&w.blocks, None, thread);
            let started = ms(t);
            dg.warm_rings();
            let rings = ms(t);
            std::hint::black_box(tables.digest());
            eprintln!(
                "{thread:?}: start {started:.2} ms, rings done {rings:.1} ms, tables ready {:.1} ms",
                ms(t)
            );
        }
    }
}

/// Fingerprint of the tables rot[1..3] then mirror[0..1] (26.3; changed by G6-F14: mirror of the
/// 16-step rotation).
const STORED_TRANSFORMS: u64 = 0xfa6c_ab13_8e6f_e4cb;

/// Mirror of a 16-step rotation (standing signs, banners, heads): left-right swaps north and
/// south (rotation 1 -> 7), front-back swaps east and west (rotation 1 -> 15); the cardinal
/// values that the mirror keeps stay put.
#[test]
fn mirror_of_sixteen_step_rotation() {
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let t = StateTransforms::new(b);
    let rot = b.prop("rotation").expect("rotation property");
    let sign = |r: i32| b.state_from_str(&format!("minecraft:oak_sign[rotation={r}]")).expect("sign state");
    use crate::template::{Mirror, Rot};
    let after = |r: i32, m: Mirror| b.get_str(t.apply(sign(r), m, Rot::None), rot).map(str::to_owned);
    for (r, lr, fb) in [(1, 7, 15), (0, 8, 0), (4, 4, 12), (8, 0, 8), (12, 12, 4), (15, 9, 1)] {
        assert_eq!(after(r, Mirror::LeftRight), Some(lr.to_string()), "left-right of {r}");
        assert_eq!(after(r, Mirror::FrontBack), Some(fb.to_string()), "front-back of {r}");
    }
}

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

/// L9-STRUCT: journal for the live world of a template placement. A block that carries data is
/// preceded by a barrier written without any side effect (flags 820), like the game, so that the
/// old block entity of the live cell goes without its removal effects and the write always lands;
/// a block without data is written alone. The copy itself never holds the barrier.
#[test]
fn template_block_entity_journals_a_barrier_first() {
    use crate::decor_region::{LiveOp, Region};
    use crate::structure_place::{Placing, place_template};
    use crate::template::{TBlock, Template};
    use rc_proto::nbt::Nbt;
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let chest = b.state_from_str("minecraft:chest").expect("chest");
    let stone = b.state_from_str("minecraft:stone").expect("stone");
    let barrier = b.state_from_str("minecraft:barrier").expect("barrier");
    let t = Template {
        size: [2, 1, 1],
        palettes: vec![vec![
            TBlock { pos: [0, 0, 0], state: stone, nbt: None },
            TBlock { pos: [1, 0, 0], state: chest, nbt: Some(Nbt::Compound(Vec::new())) },
        ]],
        cats: vec![[1, 1]],
        jigsaws: vec![Vec::new()],
        entities: Vec::new(),
    };
    let s = Placing {
        mirror: crate::template::Mirror::None,
        rot: crate::template::Rot::None,
        pivot: [0, 0, 0],
        bbox: None,
        procs: Vec::new().into(),
        waterlog: false,
        seed: None,
        shared: false,
        known_shape: true,
        entities: None,
    };
    let mut chunks: Vec<DecoChunk> = (-1..=1).flat_map(|z| (-1..=1).map(move |x| DecoChunk::new(x, z, -64, 384))).collect();
    let arr: [&mut DecoChunk; 9] = chunks.iter_mut().collect::<Vec<_>>().try_into().unwrap();
    let mut r = Region::new(arr, b, 63, 1);
    r.live_log = Some(Vec::new());
    let mut rng = crate::decor_rng::DecoRng::legacy(1);
    assert!(place_template(&mut r, b, &w.templates.transforms, 1, &t, [8, 70, 8], [8, 70, 8], &s, &mut rng, 2));
    let ops = r.live_log.take().expect("journal");
    assert_eq!(
        ops,
        vec![
            LiveOp::Set([8, 70, 8], stone, 2),
            LiveOp::Set([9, 70, 8], barrier, 820),
            LiveOp::Set([9, 70, 8], chest, 2),
        ],
        "journal of the placement"
    );
    assert_eq!(r.get_p([9, 70, 8]), chest, "the copy holds the chest");
}

/// L9-STRUCT: the six height maps of a region copy from one walk per column, started at the
/// top of the highest section that holds something (air above), equal map by map to a walk per
/// map from the top of the world: random columns (air, fluids, leaves, plants, solids).
#[test]
fn prime_many_matches_prime_per_map() {
    use crate::decor_region::Hm;
    let Some(w) = world_data() else { return };
    let b = &*w.blocks;
    let pool: Vec<u32> = [
        "minecraft:air",
        "minecraft:cave_air",
        "minecraft:water",
        "minecraft:lava",
        "minecraft:oak_leaves",
        "minecraft:stone",
        "minecraft:glass",
        "minecraft:short_grass",
        "minecraft:snow",
        "minecraft:cobweb",
        "minecraft:kelp",
    ]
    .iter()
    .map(|n| b.state_from_str(n).expect(n))
    .collect();
    let maps = [
        Hm::WorldSurfaceWg,
        Hm::WorldSurface,
        Hm::OceanFloorWg,
        Hm::OceanFloor,
        Hm::MotionBlocking,
        Hm::MotionBlockingNoLeaves,
    ];
    let mut rng = crate::decor_rng::DecoRng::legacy(20261102);
    for round in 0..24 {
        let mut dc = DecoChunk::new(0, 0, -64, 384);
        let sections = 1 + rng.next_int_bounded(24);
        let top = -64 + 16 * if round == 0 { 0 } else if round == 1 { 24 } else { sections };
        for (i, s) in dc.blocks.iter_mut().enumerate() {
            let y = -64 + (i >> 8) as i32;
            // Sparse cells: the walks go deep, and some columns stay empty.
            *s = if y >= top || rng.next_int_bounded(64) != 0 {
                pool[0]
            } else {
                pool[rng.next_int_bounded(pool.len() as i32) as usize]
            };
        }
        let mut per_map = dc.clone();
        for m in maps {
            per_map.prime(b, m);
        }
        dc.prime_many(b, &maps, top);
        for m in maps {
            assert_eq!(dc.heights[m as usize], per_map.heights[m as usize], "round {round}, map {m:?}");
        }
    }
}

/// L9-STRUCT: height maps of generated chunks (noise terrain with structures, flat) from one walk
/// per column (`PRIME_ONE_WALK`) equal to one walk per map.
#[test]
fn generated_heightmaps_one_walk_match() {
    let Some(w) = world_data() else { return };
    let all: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    for seed in [Some(1), Some(20261011), None] {
        let dg = dim(w, seed);
        let mut cache = crate::terrain::BiomeCache::default();
        dg.prepare_biomes(&mut cache, &all);
        for &(x, z) in &all {
            opt::force(&["PRIME_ONE_WALK"], Some(true));
            let a = dg.terrain(x, z, &cache, None);
            opt::force(&["PRIME_ONE_WALK"], Some(false));
            let b = dg.terrain(x, z, &cache, None);
            assert_eq!(a.heights, b.heights, "seed {seed:?}, chunk {x} {z}");
        }
    }
    opt::force(&["PRIME_ONE_WALK"], None);
}

/// Bench (loaded machine, `--ignored`): the four final height maps of 25 terrain chunks, one walk
/// per map against one walk per column (best of 20).
#[test]
#[ignore = "bench"]
fn bench_prime_final() {
    let Some(w) = world_data() else { return };
    let all: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    for seed in [Some(1), None] {
        let dg = dim(w, seed);
        let mut cache = crate::terrain::BiomeCache::default();
        dg.prepare_biomes(&mut cache, &all);
        let base: Vec<DecoChunk> = all.iter().map(|&(x, z)| dg.terrain(x, z, &cache, None)).collect();
        let mut best = [f64::MAX; 2];
        for _ in 0..20 {
            for (k, on) in [false, true].into_iter().enumerate() {
                opt::force(&["PRIME_ONE_WALK"], Some(on));
                let mut cs = base.clone();
                let t = std::time::Instant::now();
                for c in &mut cs {
                    c.prime_final_heightmaps(&w.blocks);
                }
                best[k] = best[k].min(t.elapsed().as_secs_f64() * 1e3);
                std::hint::black_box(&cs);
            }
        }
        opt::force(&["PRIME_ONE_WALK"], None);
        eprintln!(
            "{}: final height maps of 25 chunks, per map {:.2} ms, one walk {:.2} ms (loaded machine)",
            if seed.is_some() { "noise" } else { "flat" },
            best[0],
            best[1]
        );
    }
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

// ---------------------------------------------------------------- G6-F14

/// Yaw of a template entity: rotation then mirror on the wrapped angle (front-back changes the
/// sign, left-right gives 180° minus the angle), the stored angle removed once.
#[test]
fn template_entity_yaw_follows_the_game() {
    use crate::template::{Mirror, Rot, entity_yaw};
    assert_eq!(entity_yaw(30.0, Mirror::FrontBack, Rot::None), -30.0);
    assert_eq!(entity_yaw(30.0, Mirror::LeftRight, Rot::None), 150.0);
    assert_eq!(entity_yaw(30.0, Mirror::None, Rot::Cw90), 120.0);
    assert_eq!(entity_yaw(30.0, Mirror::None, Rot::Ccw90), 300.0);
    // A stored angle beyond 180° is wrapped for both terms, removed raw once.
    assert_eq!(entity_yaw(450.0, Mirror::None, Rot::None), -270.0);
}

/// Hanging entities turn their facing with the template: a painting facing south (0) rotated
/// a quarter turn clockwise faces west (1); an item frame facing north (2) mirrored front-back
/// keeps north, mirrored left-right faces south (3).
#[test]
fn template_hanging_entities_turn_their_facing() {
    use crate::template::{Mirror, Rot, place_entity_nbt};
    use rc_proto::nbt::Nbt;
    let facing = |id: &str, key: &str, v: i8, m: Mirror, r: Rot| {
        let mut f = vec![("id".to_owned(), Nbt::String(id.into())), (key.to_owned(), Nbt::Byte(v))];
        place_entity_nbt(&mut f, [0.5; 3], [0; 3], m, r);
        f.iter().find(|(k, _)| k == key).and_then(|(_, v)| v.as_i64())
    };
    assert_eq!(facing("minecraft:painting", "facing", 0, Mirror::None, Rot::Cw90), Some(1));
    assert_eq!(facing("minecraft:item_frame", "Facing", 2, Mirror::FrontBack, Rot::None), Some(2));
    assert_eq!(facing("minecraft:glow_item_frame", "Facing", 2, Mirror::LeftRight, Rot::None), Some(3));
    assert_eq!(facing("minecraft:item_frame", "Facing", 1, Mirror::LeftRight, Rot::Cw90), Some(1), "up stays up");
}

/// Static data appended by a rule merges deeply: an inner compound keeps its other keys.
#[test]
fn append_static_merges_compounds_deeply() {
    use rc_proto::nbt::Nbt;
    let c = |v: Vec<(&str, Nbt)>| v.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Vec<_>>();
    let mut dst = c(vec![("a", Nbt::Compound(c(vec![("x", Nbt::Int(1)), ("y", Nbt::Int(2))]))), ("b", Nbt::Int(5))]);
    let add = c(vec![("a", Nbt::Compound(c(vec![("y", Nbt::Int(9)), ("z", Nbt::Int(3))]))), ("b", Nbt::Int(6))]);
    crate::structure_place::merge_compound(&mut dst, &add);
    let want = c(vec![
        ("a", Nbt::Compound(c(vec![("x", Nbt::Int(1)), ("y", Nbt::Int(9)), ("z", Nbt::Int(3))]))),
        ("b", Nbt::Int(6)),
    ]);
    assert_eq!(dst, want);
}

/// The plain linear position test (`linear_pos`) measures the Manhattan distance (axis 3); the
/// axis-aligned one keeps its axis.
#[test]
fn linear_pos_rule_uses_the_manhattan_distance() {
    use crate::structure_place::{PosPred, Proc, ProcLists};
    let Some(w) = world_data() else { return };
    let lists = ProcLists::new(&w.decor);
    let rule = |kind: &str| {
        let text = format!(
            r#"{{"processors":[{{"processor_type":"minecraft:rule","rules":[{{"input_predicate":{{"predicate_type":"minecraft:always_true"}},"location_predicate":{{"predicate_type":"minecraft:always_true"}},"position_predicate":{{"predicate_type":"{kind}","axis":"x","min_chance":0.0,"max_chance":1.0,"min_dist":0,"max_dist":8}},"output_state":{{"Name":"minecraft:stone"}}}}]}}]}}"#
        );
        let j = crate::json::parse(&text).expect("json");
        match lists.get(&j, &w.blocks).first() {
            Some(Proc::Rules(r)) => match r.first().map(|r| &r.pos) {
                Some(PosPred::Linear(_, _, _, _, axis)) => Some(*axis),
                _ => None,
            },
            _ => None,
        }
    };
    assert_eq!(rule("minecraft:linear_pos"), Some(3));
    assert_eq!(rule("minecraft:axis_aligned_linear_pos"), Some(0));
}

/// A reference handed to the placement of a start (`place jigsaw`, jigsaw block) reaches the
/// processors: the high ramparts of the bastion treasure empty their blocks with a chance that
/// grows with the vertical distance to it; without it, the world generation's reference.
#[test]
fn start_reference_reaches_the_processors() {
    use crate::decor_region::Region;
    use crate::template::BBox;
    let Some(w) = world_data() else { return };
    let dg = nether(w, 1);
    let sg = dg.structures.clone().unwrap();
    let pool = "minecraft:bastion/treasure/ramparts";
    let top = "minecraft:bastion/treasure/ramparts/top_wall";
    let elements = sg.pools.get(pool).expect("ramparts pool").templates.clone();
    let wall = elements.iter().find(|e| elem_name(e) == top).expect("top wall element");
    let t = wall.template(&sg.templates, &dg.blocks).expect("top wall template");
    // A start whose first piece is the top wall (element drawn per chunk).
    let (pos, st) = t.jigsaws[0]
        .iter()
        .flat_map(|j| (0..64).map(move |i| (j.name.to_string(), [i * 16 + 8, 64, 8])))
        .find_map(|(target, pos)| {
            let st = dg.jigsaw_start(pool, &target, 0, pos)?;
            matches!(&st.pieces[0], Piece::Jigsaw(jp) if elem_name(&jp.element) == top).then_some((pos, st))
        })
        .expect("a top wall start");
    let deco = dg.deco.clone().unwrap();
    let (cx, cz) = (st.bbox.min[0] >> 4, st.bbox.min[2] >> 4);
    let place = |reference: Option<[i32; 3]>| {
        let mut chunks: Vec<DecoChunk> =
            (0..9).map(|i| DecoChunk::new(cx + i % 3 - 1, cz + i / 3 - 1, dg.min_y, dg.height)).collect();
        {
            let refs: [&mut DecoChunk; 9] = chunks.iter_mut().collect::<Vec<_>>().try_into().unwrap();
            let mut r = Region::new(refs, &dg.blocks, dg.sea_level, dg.seed);
            let mut ctx = crate::decor::Ctx::new(&mut r, &deco);
            let zone = BBox::new([cx * 16, dg.min_y, cz * 16], [cx * 16 + 15, dg.min_y + dg.height - 1, cz * 16 + 15]);
            let mut rng = crate::decor_rng::DecoRng::legacy(5);
            crate::structure_place::place_start_zone(&mut ctx, &sg, &st, &mut rng, zone, false, reference);
        }
        chunks[4].blocks.iter().filter(|&&s| !dg.blocks.is_air(s)).count()
    };
    let fb = st.pieces[0].bbox();
    let c = fb.center();
    let default = place(None);
    assert!(default > 100, "rampart placed: {default}");
    assert_eq!(place(Some([c[0], fb.min[1], c[2]])), default, "default = the world generation's reference");
    let far = place(Some([pos[0], pos[1] - 100, pos[2]]));
    assert!(far < default, "blocks emptied far from the reference: {far} vs {default}");
}
