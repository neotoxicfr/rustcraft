//! Small numeric extracts taken by the probe (seed 0): only need the game jar,
//! not the reference files.

mod common;

use rc_worldgen::Generator;
use rc_worldgen::biome::{BiomeSource, Searcher};
use rc_worldgen::noise::NormalNoise;

#[test]
fn seed_zero_extract() {
    let Some(pack) = common::data_pack() else { return };
    let root = rc_worldgen::rng::Rng::xoroshiro(0).fork_positional();
    let id = "minecraft:continentalness";
    let n = NormalNoise::from_json(&pack.json("noise", id).unwrap()).unwrap().create(&mut root.from_hash_of(id));
    assert_eq!(n.get(0.0, 0.0, 0.0).to_bits(), 0xbcdd_b989);
    assert_eq!(n.get(1.0, 2.0, 3.0).to_bits(), 0xbd4f_83b9);

    let g = Generator::new(&pack, "overworld", 0).unwrap();
    let d = g.final_density(0, 0);
    for (i, bits) in
        [(0, 0x3d19_872a), (100, 0x3e25_7199), (20_000, 0x3da8_7ecf), (49_282, 0xbdd9_84fc), (98_303, 0xbccc_c1e0)]
    {
        assert_eq!(d[i].to_bits(), bits, "density [{i}]");
    }

    let (src, _) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let names = src.names();
    let cb = src.chunk(&g, 0, 0, &mut Searcher::default());
    // Probe order: section, x, y, z.
    for (k, want) in
        [(0, "minecraft:river"), (300, "minecraft:river"), (1000, "minecraft:river"), (1535, "minecraft:forest")]
    {
        let (sec, x, y, z) = (k / 64, (k / 16) % 4, (k / 4) % 4, k % 4);
        assert_eq!(names[cb.get(x, sec * 4 + y, z) as usize], want, "quart {k}");
    }
}
