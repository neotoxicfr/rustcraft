use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Measures loading of the real cache of version 26.3 (prepared beforehand), on one
/// thread then in parallel sections.
fn load(c: &mut Criterion) {
    let path = rc_data::cache_root().join("26.3").join("data.bin");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("{} absent: run the preparation first", path.display());
        return;
    };
    // The fingerprint is re-read from the header: the bench measures decoding, not its validation.
    let tag = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    c.bench_function("decode_data_bin_1t", |b| b.iter(|| rc_data::decode_with(black_box(&bytes), tag, false).unwrap()));
    c.bench_function("decode_data_bin_parallel", |b| {
        b.iter(|| rc_data::decode_with(black_box(&bytes), tag, true).unwrap())
    });
}

/// Lookups by name: registry, entry of `minecraft:sound_event` (the biggest list
/// consulted at each sound) and block, index against linear search (`RUSTCRAFT_OPT_NAMEINDEX`).
fn names(c: &mut Criterion) {
    let path = rc_data::cache_root().join("26.3").join("data.bin");
    let Ok(bytes) = std::fs::read(&path) else { return };
    let tag = u64::from_le_bytes(bytes[12..20].try_into().unwrap());
    let d = rc_data::decode(&bytes, tag).unwrap();
    let sounds = d.registry("minecraft:sound_event").unwrap();
    // Names drawn at a fixed stride across the whole list (average of the linear cost).
    let picks: Vec<&str> = sounds.entries.iter().step_by(37).map(String::as_str).collect();
    let blocks: Vec<&str> = d.blocks.iter().step_by(23).map(|b| b.name.as_str()).collect();
    for indexed in [false, true] {
        let tag = if indexed { "index" } else { "linear" };
        c.bench_function(&format!("registry_{tag}"), |b| {
            b.iter(|| d.registry_with(black_box("minecraft:sound_event"), indexed).is_some())
        });
        c.bench_function(&format!("sound_id_{tag}"), |b| {
            b.iter(|| picks.iter().filter_map(|n| sounds.id_with(black_box(n), indexed)).sum::<u32>())
        });
        c.bench_function(&format!("block_id_{tag}"), |b| {
            b.iter(|| blocks.iter().filter_map(|n| d.block_id_with(black_box(n), indexed)).sum::<u32>())
        });
    }
}

criterion_group!(benches, load, names);
criterion_main!(benches);
