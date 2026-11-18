//! Micro-bench of loading the generation data (`WorldData`): old read of the jar by
//! each reader against the process's jar index, then sharing (`WorldData::shared`). Then the JSON
//! reader on the whole pack (compact tree against the old allocating one: time, allocations,
//! heap held by the trees), the pack's parse cache, and the density functions and noises parsed
//! one file after the other or on 1, 2, 4… threads.
//!
//! `cargo run --release -p rc-worldgen --example worlddata [-- repetitions]`

// Counting allocator (bench only): delegates as is to the client's allocator.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::chunkgen::WorldData;
use rc_worldgen::data::DataPack;
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_data::DecorData;
use rc_worldgen::json::{Json, parse_mode};

/// The client's allocator, with the blocks allocated and the bytes held counted.
struct Counting;

static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
static HELD_BYTES: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        HELD_BYTES.fetch_add(l.size(), Relaxed);
        unsafe { mimalloc::MiMalloc.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        HELD_BYTES.fetch_sub(l.size(), Relaxed);
        unsafe { mimalloc::MiMalloc.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Relaxed);
        HELD_BYTES.fetch_add(n, Relaxed);
        HELD_BYTES.fetch_sub(l.size(), Relaxed);
        unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// (min, median) of `reps` timed runs of `f`, in ms.
fn time_reps(reps: usize, mut f: impl FnMut()) -> (f64, f64) {
    let mut v: Vec<f64> = (0..reps)
        .map(|_| {
            let t = Instant::now();
            f();
            ms(t)
        })
        .collect();
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

fn reader_name(compact: bool) -> &'static str {
    if compact { "compact" } else { "allocating" }
}

/// Thread counts 1, 2, 4… up to the machine's logical cores.
fn thread_counts() -> Vec<usize> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut v: Vec<usize> = std::iter::successors(Some(1), |n| Some(n * 2)).take_while(|&n| n < cores).collect();
    v.push(cores);
    v
}

fn on_threads<R: Send>(threads: usize, f: impl FnOnce() -> R + Send) -> R {
    rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("thread pool").install(f)
}

/// Every file of the pack parsed by both readers: time, allocations of a pass, heap held by the
/// trees of every file at once; then the compact reader on 1, 2, 4… threads (one file per task).
fn bench_whole_pack(pack: &DataPack, reps: usize) {
    let files: Vec<&str> = pack.texts().map(|(_, t)| t).collect();
    let bytes: usize = files.iter().map(|t| t.len()).sum();
    println!("pack: {} files, {:.2} MB of text", files.len(), bytes as f64 / 1e6);
    for compact in [false, true] {
        let (a, m) = time_reps(reps, || files.iter().for_each(|t| drop(std::hint::black_box(parse_mode(t, compact)))));
        let before = (ALLOCATIONS.load(Relaxed), HELD_BYTES.load(Relaxed));
        let trees: Vec<Json> = files.iter().map(|t| parse_mode(t, compact).unwrap()).collect();
        let allocations = ALLOCATIONS.load(Relaxed) - before.0;
        let held = HELD_BYTES.load(Relaxed) - before.1;
        drop(trees);
        let name = reader_name(compact);
        println!("parse every file, {name:10} reader     : {a:8.1} / {m:8.1} ms, {allocations} allocations, {:.1} MB held", held as f64 / 1e6);
    }
    for threads in thread_counts() {
        let (a, m) = on_threads(threads, || {
            time_reps(reps, || files.par_iter().for_each(|t| drop(std::hint::black_box(parse_mode(t, true)))))
        });
        println!("parse every file, compact, {threads:2} threads    : {a:8.1} / {m:8.1} ms");
    }
}

/// The largest file (the overworld biome report) by both readers, then the climate list built
/// from it (reader of the current switches).
fn bench_biome_report(pack: &DataPack, reps: usize) {
    let Some(report) = pack.raw("biome_parameters", "minecraft:overworld") else {
        println!("overworld biome report absent (no reports folder)");
        return;
    };
    println!("overworld biome report: {:.2} MB", report.len() as f64 / 1e6);
    for compact in [false, true] {
        let (a, m) = time_reps(reps, || drop(std::hint::black_box(parse_mode(report, compact))));
        println!("parse the report, {:10} reader       : {a:8.1} / {m:8.1} ms", reader_name(compact));
    }
    let (a, m) = time_reps(reps, || {
        std::hint::black_box(rc_worldgen::biome::ClimateList::from_report(pack, "minecraft:overworld").unwrap());
    });
    println!("climate list from the report (parse + build) : {a:8.1} / {m:8.1} ms");
}

/// Every biome file read through the pack twice: the second read finds the tree of the first
/// when the parse cache is on (`RUSTCRAFT_OPT_JSON_PARSE_CACHE`, default).
fn bench_parse_cache(pack: &DataPack) {
    let ids = pack.ids("biome");
    let read_all = || {
        let t = Instant::now();
        ids.iter().for_each(|id| drop(std::hint::black_box(pack.json("biome", id).unwrap())));
        ms(t)
    };
    let before = HELD_BYTES.load(Relaxed);
    let (first, second) = (read_all(), read_all());
    let kept = HELD_BYTES.load(Relaxed) as f64 - before as f64;
    let mode = if rc_worldgen::opt::json_parse_cache() { "on" } else { "off" };
    println!(
        "{} biome files read twice, parse cache {mode:3}  : {first:8.2} then {second:8.3} ms, {:.2} MB kept",
        ids.len(),
        kept / 1e6
    );
}

/// The three dimension generators (with structures) built then dropped: time, and the heap the
/// world data still holds afterwards (the parse cache's trees, the shared lists); run once with
/// `RUSTCRAFT_OPT_JSON_PARSE_CACHE=0` for the share of the cache.
fn bench_three_generators(w: &WorldData) {
    use rc_worldgen::chunkgen::{DimGen, GenSpec, SourceSpec};
    let noise = |settings: &str, source: SourceSpec| GenSpec::Noise { settings: settings.into(), source };
    let dims = [
        ("minecraft:overworld", noise("minecraft:overworld", SourceSpec::MultiNoise("minecraft:overworld".into()))),
        ("minecraft:the_nether", noise("minecraft:nether", SourceSpec::MultiNoise("minecraft:nether".into()))),
        ("minecraft:the_end", noise("minecraft:end", SourceSpec::TheEnd)),
    ];
    let before = HELD_BYTES.load(Relaxed);
    let t = Instant::now();
    for (dim, spec) in &dims {
        let mut g = DimGen::new(w, spec, dim, 1234).expect("generator");
        g.enable_structures(w).expect("structures");
    }
    let held = HELD_BYTES.load(Relaxed) as f64 - before as f64;
    let mode = if rc_worldgen::opt::json_parse_cache() { "on" } else { "off" };
    println!("three generators built, parse cache {mode:3}  : {:8.1} ms, {:.2} MB still held", ms(t), held / 1e6);
}

/// Density functions and noises of the pack, one file after the other or on 1, 2, 4… threads.
fn bench_defs_load(pack: &DataPack, reps: usize) {
    let series = time_reps(reps, || drop(std::hint::black_box(rc_worldgen::generator::Defs::load_mode(pack, false))));
    println!("density functions and noises, in series      : {:8.2} / {:8.2} ms", series.0, series.1);
    for threads in thread_counts() {
        let (a, m) = on_threads(threads, || {
            time_reps(reps, || drop(std::hint::black_box(rc_worldgen::generator::Defs::load_mode(pack, true))))
        });
        println!("density functions and noises, {threads:2} threads   : {a:8.2} / {m:8.2} ms");
    }
}

fn main() {
    let reps: usize = std::env::args().nth(1).and_then(|r| r.parse().ok()).unwrap_or(5);
    let jar = rc_data::locate("26.3").expect("jar 26.3").client_jar;
    let v = rc_data::load_cached("26.3").expect("data 26.3");
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    let reports = reports.is_dir().then_some(reports.as_path());
    let old = || {
        let mut pack = DataPack::from_jar_direct(&jar).unwrap();
        if let Some(r) = reports {
            pack.add_reports(r).unwrap();
        }
        let decor = DecorData::from_jar_direct(&jar).unwrap();
        let blocks = Blocks::new(&v, &decor).unwrap();
        let t = rc_worldgen::template::Templates::new(decor.files_of("template"), &blocks);
        std::hint::black_box((pack, decor, blocks, t));
    };
    let t = Instant::now();
    let first = WorldData::load(&jar, reports, &v).unwrap();
    println!("index (1st call: jar indexed) + load         : {:8.1} ms", ms(t));
    bench_three_generators(&first);
    bench_whole_pack(&first.pack, reps);
    bench_biome_report(&first.pack, reps);
    bench_parse_cache(&first.pack);
    bench_defs_load(&first.pack, reps);
    drop(first);
    let o = time_reps(reps, old);
    println!("old load (jar read twice)                    : {:8.1} / {:8.1} ms", o.0, o.1);
    let n = time_reps(reps, || drop(std::hint::black_box(WorldData::load(&jar, reports, &v).unwrap())));
    println!("load from the index                          : {:8.1} / {:8.1} ms", n.0, n.1);
    let keep = WorldData::shared(&jar, reports, &v).unwrap();
    let s = time_reps(reps, || {
        let again = WorldData::shared(&jar, reports, &v).unwrap();
        assert!(Arc::ptr_eq(&keep, &again));
    });
    println!("shared data already held                     : {:8.3} / {:8.3} ms", s.0, s.1);
}
