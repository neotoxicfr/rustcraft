//! Census of the PCM cache (I-05): decodes every non-streamed variant of the table, then
//! estimates the cache of a long survival game (event families heard on the surface,
//! in caves, in the rain, with the usual mobs). Measurement ahead of any cap:
//! the cache keeps every sound played until the resources reload, like the game.
//! `cargo-file.ps1 run --release -p rc-audio --example pcm_census`

use std::collections::{BTreeMap, HashSet};
use std::time::Instant;

use rc_audio::SoundTable;
use rc_audio::sounds::VariantKind;

/// Families of a long survival game (event prefixes, without `minecraft:`).
const SESSION: &[&str] = &[
    "ambient.cave",
    "weather.rain",
    "ambient.underwater.",
    "block.stone.",
    "block.grass.",
    "block.gravel.",
    "block.sand.",
    "block.wood.",
    "block.deepslate.",
    "block.water.",
    "block.lava.",
    "block.chest.",
    "block.wooden_door.",
    "block.furnace.",
    "block.anvil.",
    "block.ladder.",
    "block.wool.",
    "block.glass.",
    "block.metal.",
    "block.copper.",
    "block.tuff.",
    "block.cobweb.",
    "entity.player.",
    "entity.generic.",
    "entity.item.",
    "entity.experience_orb.",
    "entity.arrow.",
    "entity.zombie.",
    "entity.skeleton.",
    "entity.creeper.",
    "entity.spider.",
    "entity.enderman.",
    "entity.cow.",
    "entity.pig.",
    "entity.sheep.",
    "entity.chicken.",
    "entity.villager.",
    "entity.bat.",
    "entity.slime.",
    "entity.witch.",
    "ui.button.click",
];

fn files(table: &SoundTable, id: &str, depth: u32, out: &mut HashSet<String>) {
    if depth > 16 {
        return;
    }
    let Some(e) = table.get(id) else { return };
    for v in &e.variants {
        match &v.kind {
            VariantKind::File(p) if !v.stream => {
                out.insert(p.clone());
            }
            VariantKind::File(_) => {}
            VariantKind::Event(t) => files(table, t, depth + 1, out),
        }
    }
}

fn main() {
    let install = rc_data::locate("26.3").expect("installation 26.3");
    let index = install.asset_index();
    let assets = rc_assets::Assets::for_install(
        &install.client_jar,
        &[],
        index.as_ref().map(|(i, o)| (i.as_path(), o.as_path())),
    )
    .expect("resources");
    let table = SoundTable::load(&[&assets]);
    let mut ids: Vec<&str> = table.ids().collect();
    ids.sort_unstable();
    let mut all = HashSet::new();
    for id in &ids {
        files(&table, id, 0, &mut all);
    }
    let mut all: Vec<String> = all.into_iter().collect();
    all.sort_unstable();
    // PCM size (bytes) and duration (ms) of each file.
    let mut size = BTreeMap::new();
    let t = Instant::now();
    for p in &all {
        let Some(b) = assets.read(p) else { continue };
        if let Ok(pcm) = rc_audio::decode::decode_all(b) {
            size.insert(p.clone(), (pcm.data.len() * 2, f64::from(pcm.seconds()) * 1e3));
        }
    }
    let total: usize = size.values().map(|s| s.0).sum();
    println!(
        "{} non-streamed files, PCM {:.1} MB, decoded in {:.1} s",
        size.len(),
        total as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
    for limit in [250.0, 500.0, 1000.0, 2000.0] {
        let (n, b) = size.values().filter(|s| s.1 < limit).fold((0, 0), |(n, b), s| (n + 1, b + s.0));
        println!("  < {limit} ms: {n} files, {:.1} MB", b as f64 / 1e6);
    }
    let mut session = HashSet::new();
    let mut per_family = Vec::new();
    for fam in SESSION {
        let mut f = HashSet::new();
        for id in ids.iter().filter(|i| i.strip_prefix("minecraft:").is_some_and(|s| s.starts_with(fam))) {
            files(&table, id, 0, &mut f);
        }
        let b: usize = f.iter().filter_map(|p| size.get(p)).map(|s| s.0).sum();
        per_family.push((b, *fam, f.len()));
        session.extend(f);
    }
    per_family.sort_unstable_by(|a, b| b.cmp(a));
    let sb: usize = session.iter().filter_map(|p| size.get(p)).map(|s| s.0).sum();
    println!("sample survival game: {} files, {:.1} MB (every variant heard)", session.len(), sb as f64 / 1e6);
    for (b, fam, n) in per_family.iter().take(12) {
        println!("  {fam:<24} {n:>4} files {:>7.2} MB", *b as f64 / 1e6);
    }
    for limit in [1000.0, 1500.0, 2000.0, 3000.0] {
        let long: Vec<_> = session.iter().filter_map(|p| size.get(p)).filter(|s| s.1 >= limit).collect();
        let b: usize = long.iter().map(|s| s.0).sum();
        println!("  of which {} sounds of {limit} ms or more: {:.1} MB", long.len(), b as f64 / 1e6);
    }
    // Time to reload a forgotten sound (decoding only, 1 thread, 3 passes, best one kept).
    let mut times: Vec<(f64, f64, &str)> = Vec::new();
    for p in &session {
        let Some(&(_, ms)) = size.get(p) else { continue };
        if ms < 2000.0 {
            continue;
        }
        let b = assets.read(p).expect("sound");
        let best = (0..3)
            .map(|_| {
                let t = Instant::now();
                let _ = rc_audio::decode::decode_all(&b);
                t.elapsed().as_secs_f64() * 1e3
            })
            .fold(f64::INFINITY, f64::min);
        times.push((best, ms, p));
    }
    times.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let q = |f: f64| times[((times.len() - 1) as f64 * f) as usize];
    println!(
        "decoding of the {} sounds of 2 s or more: median {:.1} ms, p99 {:.1} ms, max {:.1} ms ({:.0} ms of sound, {})",
        times.len(),
        q(0.5).0,
        q(0.99).0,
        q(1.0).0,
        q(1.0).1,
        q(1.0).2
    );
}
