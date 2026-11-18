//! Creates a new world (test tool for world generation):
//! `cargo run --release -p rc-sim --example new_world -- <folder> [seed] [normal|flat|large_biomes|amplified]`

use std::sync::Arc;

use rc_sim::newworld::{NewWorld, Preset, create_world, seed_from_text};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("usage: new_world <folder> [seed] [preset]");
        std::process::exit(2);
    };
    let data = Arc::new(rc_data::load_or_prepare("26.3", |s| eprintln!("data: {s}")).expect("version data"));
    let dir = std::path::PathBuf::from(dir);
    let mut w = NewWorld::new(&dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    w.seed = seed_from_text(args.get(1).map(String::as_str).unwrap_or(""));
    w.preset = match args.get(2).map(String::as_str) {
        Some("flat") => Preset::Flat,
        Some("large_biomes") => Preset::LargeBiomes,
        Some("amplified") => Preset::Amplified,
        _ => Preset::Normal,
    };
    w.game_mode = 1;
    w.allow_commands = true;
    let t = std::time::Instant::now();
    match create_world(&dir, &data, &w) {
        Ok(()) => {
            println!("world created in {:.2} s (seed {})", t.elapsed().as_secs_f64(), w.seed);
            // 4th argument "x,y,z,pitch": forced spawn point (inspection views).
            if let Some(v) = args.get(3) {
                use rc_proto::nbt::Nbt;
                use rc_sim::nbtio;
                let n: Vec<f32> = v.split(',').filter_map(|x| x.parse().ok()).collect();
                let path = dir.join("level.dat");
                let mut root = nbtio::read_dat(&path).expect("level.dat");
                let d = nbtio::get_mut(&mut root, "Data").unwrap();
                let sp = nbtio::get_mut(d, "spawn").unwrap();
                nbtio::set(sp, "pos", Nbt::IntArray(vec![n[0] as i32, n[1] as i32, n[2] as i32]));
                nbtio::set(sp, "pitch", Nbt::Float(n[3]));
                std::fs::write(&path, nbtio::gzip(&nbtio::encode_root(&root))).unwrap();
            }
        }
        Err(e) => {
            eprintln!("failed: {e}");
            std::process::exit(1);
        }
    }
}
