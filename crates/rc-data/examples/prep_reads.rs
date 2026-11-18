//! Duration of the preparation's jar reads (4 folders of the base pack), old path (one
//! opening per folder) or single pass (shared index, cold: one process per measurement).
//! `cargo run -p rc-data --example prep_reads --release -- old|single`

use std::time::Instant;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode == "compile" {
        // Compilation from the real cache (`RUSTCRAFT_OPT_PREPPAR=0/1`: item reports in series or not).
        let dir = rc_data::cache_root().join("26.3");
        let t = Instant::now();
        let d = rc_data::compile(&dir.join("facts.json"), &dir.join("generated").join("reports"), 777).unwrap();
        println!("compile: {} items in {:.1} ms", d.items.len(), t.elapsed().as_secs_f64() * 1e3);
        return;
    }
    let single = mode == "single";
    let jar = rc_data::locate("26.3").expect("installation 26.3").client_jar;
    let t = Instant::now();
    let mut n = 0;
    for folder in ["tags/timeline", "dimension_type", "timeline", "worldgen/biome"] {
        n += rc_data::data_jsons_with(&jar, folder, single).unwrap().len();
    }
    println!(
        "{} : {n} JSON en {:.1} ms",
        if single { "single pass" } else { "old" },
        t.elapsed().as_secs_f64() * 1e3
    );
}
