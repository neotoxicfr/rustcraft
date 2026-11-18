//! Biomes compared with the game: quantised parameter lists, climate and biome of each quart of
//! several chunks (overworld, large biomes, Nether, End), block zoom.

mod common;

use std::sync::Arc;

use rc_worldgen::Ctx;
use rc_worldgen::biome::{BiomeSource, ClimateList, Searcher};
use rc_worldgen::generator::{Defs, NoiseSettings};
use rc_worldgen::{Generator, Volume};

#[test]
fn parameter_lists_match_game() {
    let Some(pack) = common::data_pack() else { return };
    for preset in ["overworld", "nether"] {
        let Some(lines) = common::read_lines(&format!("params_{preset}.txt")) else { return };
        let list = ClimateList::from_report(&pack, preset).unwrap();
        assert_eq!(list.entries.len(), lines.len(), "{preset}");
        for (i, l) in lines.iter().enumerate() {
            let t: Vec<&str> = l.split(' ').collect();
            let (space, b) = &list.entries[i];
            assert_eq!(list.names[*b as usize], t[0], "{preset} entry {i}");
            let v: Vec<i64> = t[1..].iter().map(|s| s.parse().unwrap()).collect();
            for d in 0..6 {
                assert_eq!(space[d], (v[2 * d], v[2 * d + 1]), "{preset} entry {i} dimension {d}");
            }
            assert_eq!(space[6], (v[12], v[12]), "{preset} entry {i} offset");
        }
    }
}

/// Overlaps of entries (zero distance for the same target): the only cases where the tie
/// rule of the tree can matter (§ 7.3). Measurement, printed.
#[test]
fn overlapping_entries_census() {
    let Some(pack) = common::data_pack() else { return };
    for preset in ["overworld", "nether"] {
        let list = ClimateList::from_report(&pack, preset).unwrap();
        let e = &list.entries;
        let mut overlaps = 0usize;
        let mut differing = 0usize;
        for i in 0..e.len() {
            for j in i + 1..e.len() {
                if (0..7).all(|d| e[i].0[d].0 <= e[j].0[d].1 && e[j].0[d].0 <= e[i].0[d].1) {
                    overlaps += 1;
                    if e[i].1 != e[j].1 {
                        differing += 1;
                    }
                }
            }
        }
        eprintln!(
            "{preset}: {} entries, {overlaps} overlapping pairs, of which {differing} of different biomes",
            e.len()
        );
    }
}

fn check_file(defs: &Arc<Defs>, preset: &str, seed: i64, source: &BiomeSource) -> Vec<String> {
    let pack = common::data_pack().unwrap();
    let g = Generator::with_defs(defs.clone(), NoiseSettings::load(&pack, preset).unwrap(), seed).unwrap();
    let names = source.names();
    let lines = common::read_lines(&format!("biomes_{preset}_{seed}.txt")).unwrap();
    let mut searcher = Searcher::default();
    let mut failures = Vec::new();
    let mut quarts = 0usize;
    for l in &lines {
        let t: Vec<&str> = l.split(' ').collect();
        let (cx, cz): (i32, i32) = (t[1].parse().unwrap(), t[2].parse().unwrap());
        let min_qy = g.settings.min_y >> 2;
        let hq = g.settings.height >> 2;
        match t[0] {
            "chunk" => {
                let cb = source.chunk(&g, cx, cz, &mut searcher);
                let mut k = 3;
                for sec in 0..hq / 4 {
                    for x in 0..4 {
                        for y in 0..4 {
                            for z in 0..4 {
                                let got = &names[cb.get(x, sec * 4 + y, z) as usize];
                                if got != t[k] {
                                    failures.push(format!(
                                        "{preset} {seed} chunk {cx},{cz} quart {x},{},{z}: {got} ≠ {}",
                                        min_qy + sec * 4 + y,
                                        t[k]
                                    ));
                                }
                                k += 1;
                                quarts += 1;
                            }
                        }
                    }
                }
            }
            "climate" => {
                // Same context as the filling: climate volumes computed first.
                let mut ctx = Ctx::cached();
                let v = Volume::stepped(4, hq, 4, cx * 16, min_qy << 2, cz * 16, 4, 4, 4);
                let r = g.router;
                for s in [r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges] {
                    g.volume(s, &mut ctx, &v);
                }
                let mut k = 3;
                for sec in 0..hq / 4 {
                    for x in 0..4 {
                        for y in 0..4 {
                            for z in 0..4 {
                                let c =
                                    BiomeSource::climate_at(&g, &mut ctx, cx * 4 + x, min_qy + sec * 4 + y, cz * 4 + z);
                                let want: Vec<i64> = t[k].split(',').map(|s| s.parse().unwrap()).collect();
                                if c[..] != want[..] {
                                    failures.push(format!(
                                        "{preset} {seed} climate {cx},{cz} quart {x},{y},{z}: {c:?} ≠ {want:?}"
                                    ));
                                }
                                k += 1;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    eprintln!("{preset} {seed}: {quarts} quarts compared");
    failures
}

#[test]
fn chunk_biomes_match_game() {
    let Some(pack) = common::data_pack() else { return };
    if common::ref_dir().is_none() {
        return;
    }
    let defs = Arc::new(Defs::load(&pack).unwrap());
    let (overworld, settings) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    assert_eq!(settings, "minecraft:overworld");
    let (nether, _) = BiomeSource::from_preset(&pack, "normal", "the_nether").unwrap();
    let (end, _) = BiomeSource::from_preset(&pack, "normal", "the_end").unwrap();
    let mut failures = Vec::new();
    for seed in [0i64, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789] {
        failures.extend(check_file(&defs, "overworld", seed, &overworld));
    }
    failures.extend(check_file(&defs, "large_biomes", 0, &overworld));
    failures.extend(check_file(&defs, "nether", 0, &nether));
    failures.extend(check_file(&defs, "end", 0, &end));
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures[..failures.len().min(30)].join("\n"));
}

#[test]
fn block_zoom_matches_game() {
    let Some(lines) = common::read_lines("zoom.txt") else { return };
    for l in &lines {
        let v: Vec<i64> = l.split(' ').map(|s| s.parse().unwrap()).collect();
        let q = rc_worldgen::biome::zoom_world(v[0], v[1] as i32, v[2] as i32, v[3] as i32);
        assert_eq!((i64::from(q.0), i64::from(q.1), i64::from(q.2)), (v[4], v[5], v[6]), "{l}");
    }
    assert!(lines.len() > 4000);
}

#[test]
fn tree_agrees_with_brute_force() {
    let Some(pack) = common::data_pack() else { return };
    let list = ClimateList::from_report(&pack, "overworld").unwrap();
    let mut r = rc_worldgen::rng::Rng::xoroshiro(7);
    let mut s = Searcher::default();
    let mut diff = 0;
    for _ in 0..20_000 {
        let t = [0; 6].map(|_| i64::from(r.next_int_bounded(24_000)) - 12_000);
        let e = list.find_entry(t, &mut s);
        let brute = list.find_brute_force(t);
        if list.entries[e].1 != brute {
            diff += 1;
        }
        // The tree always reaches the minimal distance; only the choice among ties differs.
        let best = (0..list.entries.len()).map(|i| list.distance(i, t)).min().unwrap();
        assert_eq!(list.distance(e, t), best, "{t:?}");
    }
    eprintln!("random targets: tree biome ≠ first tie of the list: {diff} / 20000");
    // Real ties on the climate of the reference chunks (seed 0).
    let Some(lines) = common::read_lines("biomes_overworld_0.txt") else { return };
    let (mut n, mut tied) = (0, 0);
    for l in lines.iter().filter(|l| l.starts_with("climate")) {
        for c in l.split(' ').skip(3) {
            let v: Vec<i64> = c.split(',').map(|x| x.parse().unwrap()).collect();
            n += 1;
            if list.tied_biomes([v[0], v[1], v[2], v[3], v[4], v[5]]) > 1 {
                tied += 1;
            }
        }
    }
    eprintln!("real climates: {tied} / {n} quarts with several biomes at minimal distance");
}
