//! Micro-bench of loading the generation data (`WorldData`): old read of the jar by
//! each reader against the process's jar index, then sharing (`WorldData::shared`).
//!
//! `cargo run --release -p rc-worldgen --example worlddata [-- repetitions]`

use std::sync::Arc;
use std::time::Instant;

use rc_worldgen::chunkgen::WorldData;
use rc_worldgen::data::DataPack;
use rc_worldgen::decor_blocks::Blocks;
use rc_worldgen::decor_data::DecorData;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
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
    drop(first);
    let mut o = Vec::new();
    let mut n = Vec::new();
    let mut s = Vec::new();
    for _ in 0..reps {
        let t = Instant::now();
        old();
        o.push(ms(t));
        let t = Instant::now();
        std::hint::black_box(WorldData::load(&jar, reports, &v).unwrap());
        n.push(ms(t));
        let keep = WorldData::shared(&jar, reports, &v).unwrap();
        let t = Instant::now();
        let again = WorldData::shared(&jar, reports, &v).unwrap();
        s.push(ms(t));
        assert!(Arc::ptr_eq(&keep, &again));
    }
    let med = |mut v: Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[0], v[v.len() / 2])
    };
    let (a, m) = med(o);
    println!("old load (jar read twice)                    : {a:8.1} / {m:8.1} ms");
    let (a, m) = med(n);
    println!("load from the index                          : {a:8.1} / {m:8.1} ms");
    let (a, m) = med(s);
    println!("shared data already held                     : {a:8.3} / {m:8.3} ms");
}
