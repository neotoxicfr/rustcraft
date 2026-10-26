// Direct read allowed (test: settings read at launch, off the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Regeneration below y = 0 of old converted chunks, against the official server.
//!
//! `RUSTCRAFT_RETRO_IN`: input world (chunks with status `empty` and `below_zero_retrogen`);
//! `RUSTCRAFT_RETRO_REF`: the same world after a pass of the official server 26.3 (forced chunks).
//! Absent: test skipped. The converted chunks of the reference are compared block by block, above
//! and below y = 0 separately: above, the old blocks must be intact.

use hashbrown::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_sim::generation::DecorOrder;
use rc_sim::generation_nbt::Conv;
use rc_sim::states::States;
use rc_sim::world::Dimension;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let q = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &q);
        } else {
            std::fs::copy(&p, &q).unwrap();
        }
    }
}

#[test]
fn converted_chunks_match_official_retrogen() {
    let (Some(input), Some(reference)) = (
        std::env::var_os("RUSTCRAFT_RETRO_IN").map(PathBuf::from),
        std::env::var_os("RUSTCRAFT_RETRO_REF").map(PathBuf::from),
    ) else {
        return;
    };
    let Some(data) = rc_testdata::version_data() else { return };
    let data = Arc::new(data);
    let dim = "minecraft:overworld";
    let jar = rc_sim::registries::client_jar(&data).expect("jar");
    let regs = rc_sim::registries::load(&jar, &data).expect("registries");
    let biomes = regs.entries("minecraft:worldgen/biome").to_vec();
    let states = States::new(&data, biomes.clone());
    let height = rc_sim::dimension_height(&data, dim);
    let indim = Dimension::new(&input, dim, height);
    let mut converted: Vec<(i32, i32)> = Vec::new();
    for (rx, rz) in indim.regions() {
        let list: Vec<(i32, i32)> = (0..1024).map(|i| ((rx << 5) + (i & 31), (rz << 5) + (i >> 5))).collect();
        for (p, n) in indim.load_raw("region", &list) {
            if n.is_some_and(|n| n.get("below_zero_retrogen").is_some()) {
                converted.push(p);
            }
        }
    }
    assert!(!converted.is_empty(), "no converted chunk in the input");
    let work = std::env::temp_dir().join(format!("rc-retro-ref-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    copy_dir(&input, &work);
    let (full, _) =
        rc_sim::generation::generate_now(data.clone(), &work, dim, biomes.clone(), &converted, DecorOrder::Classes)
            .expect("generation");
    let blocks = rc_worldgen::decor_blocks::Blocks::new(
        &data,
        &rc_worldgen::decor_data::DecorData::from_jar(&jar).expect("decoration"),
    )
    .expect("blocks");
    let conv = Conv::new(&data, &states, &blocks, height, &biomes);
    let refdim = Dimension::new(&reference, dim, height);
    let name = |s: u32| blocks.block_name(blocks.block(s)).to_owned();
    let (mut above, mut below, mut biome_bad, mut lost, mut bedrock_left) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut pairs: HashMap<(bool, String, String), usize> = HashMap::new();
    for p in &converted {
        let c = full.iter().find(|c| (c.x, c.z) == *p).expect("converted chunk returned full");
        let (mine, _, _) = conv.from_nbt(&c.root).expect("reread");
        let raw = refdim.load_raw("region", &[*p]).pop().and_then(|(_, n)| n).expect("reference");
        assert_eq!(raw.get("Status").and_then(rc_proto::nbt::Nbt::as_str), Some("minecraft:full"), "{p:?}");
        let (theirs, _, _) = conv.from_nbt(&raw).expect("readable reference");
        let raw_in = indim.load_raw("region", &[*p]).pop().and_then(|(_, n)| n).expect("input");
        let (old, _, _) = conv.from_nbt(&raw_in).expect("readable input");
        for (i, ((a, b), o)) in mine.blocks.iter().zip(&theirs.blocks).zip(&old.blocks).enumerate() {
            let y = (i >> 8) as i32 + mine.min_y;
            // Old full block kept by the game but lost on our side: destruction.
            if y >= 0 && b == o && a != o && blocks.is_air(*a) && !blocks.is_air(*o) {
                lost += 1;
            }
            if (0..=4).contains(&y) && name(*o) == "bedrock" && name(*a) == "bedrock" {
                bedrock_left += 1;
            }
            if a != b {
                if y >= 0 {
                    above += 1;
                    if above <= 40 {
                        let (x, z) = ((i & 15) as i32, ((i >> 4) & 15) as i32);
                        eprintln!("  up {p:?} ({x}, {y}, {z}) us {} / them {}", name(*a), name(*b));
                    }
                } else {
                    below += 1;
                }
                *pairs.entry((y >= 0, name(*a), name(*b))).or_default() += 1;
            }
        }
        biome_bad += mine.biomes.iter().zip(&theirs.biomes).filter(|(a, b)| a != b).count();
    }
    let mut v: Vec<_> = pairs.into_iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1));
    eprintln!(
        "{} converted chunks: {above} differences above 0, {below} below, {biome_bad} biome cells, \
         {lost} old blocks lost, {bedrock_left} old bedrocks left",
        converted.len()
    );
    for ((up, a, b), n) in v.iter().take(80) {
        eprintln!("  {} us {a} / them {b}: {n}", if *up { "up  " } else { "down" });
    }
    let _ = std::fs::remove_dir_all(&work);
    assert_eq!((lost, bedrock_left, biome_bad), (0, 0, 0), "old blocks destroyed, bedrock or biomes");
}
