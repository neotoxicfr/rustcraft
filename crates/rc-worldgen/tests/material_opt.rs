//! Bit-for-bit equivalence of the optimised paths of the material rules (`MatOpts`, switches
//! `RUSTCRAFT_OPT_ORE_IDX`, `ORE_EARLY`, `SURF_SKIP`, `SURF_BANDS`, `VEIN_YRANGE`,
//! `VEIN_LAZYRICH`, `VEIN_SHARED`, `MAT_REUSE`, `MAT_COLCACHE`, and the volume's `VOL_POW2`) against
//! the old code: 64 chunks (surface then caves) with a stored SHA-256 fingerprint, restricted
//! vein buffers against full ones, modified rule pack, volume index. Missing game
//! data makes the tests fail under `RUSTCRAFT_OPT_REQUIRE=1`.

mod common;

use rayon::prelude::*;
use rc_worldgen::biome::{BiomeSource, ChunkBiomes, Searcher, zoom_cached};
use rc_worldgen::data::DataPack;
use rc_worldgen::material::{
    BandKernel, BiomeView, Env, MatOpts, TEMPERATURE_NOISE_BOUND, possible_biomes, record_top_material_calls,
};
use rc_worldgen::noise::Simplex;
use rc_worldgen::proto::ProtoTerrain;
use rc_worldgen::rng::Rng;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator};
use rc_worldgen::{Ctx, Volume};

/// splitmix64 for the test draws.
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        (i64::from(lo) + (self.next() % (i64::from(hi) - i64::from(lo)) as u64) as i64) as i32
    }
}

const SEED: i64 = 20_261_011;

/// Fingerprint of the 64 chunks (old path, surface then caves), block names included.
const EXPECTED: &str = "909b45abb4118b6311c6523afa8c9df22b7d0e05853e385d6744ae386c0c740e";

/// The new paths of the first material work, without the band kernels and what came with them.
const EARLIER_PATHS: MatOpts = MatOpts {
    band_kernels: false,
    row_kernel: false,
    direct_solid_write: false,
    vein_top_bound: false,
    cave_top_column_memo: false,
    ..MatOpts::NEW
};

/// Each switch alone, the dependencies, combinations, then all.
fn variants() -> Vec<(&'static str, MatOpts)> {
    let o = MatOpts::OLD;
    let n = MatOpts::NEW;
    vec![
        ("ore_idx", MatOpts { ore_idx: true, ..o }),
        ("ore_early", MatOpts { ore_early: true, ..o }),
        ("surf_skip", MatOpts { surf_skip: true, ..o }),
        ("surf_bands", MatOpts { surf_bands: true, ..o }),
        ("vein_yrange", MatOpts { vein_yrange: true, ..o }),
        ("vein_yrange+lazyrich", MatOpts { vein_yrange: true, vein_lazyrich: true, ..o }),
        ("vein_yrange+shared", MatOpts { vein_yrange: true, vein_shared: true, ..o }),
        ("reuse", MatOpts { reuse: true, ..o }),
        ("colcache", MatOpts { colcache: true, ..o }),
        ("band_kernels", MatOpts { band_kernels: true, ..o }),
        ("band_kernels+row_kernel", MatOpts { band_kernels: true, row_kernel: true, ..o }),
        ("direct_solid_write", MatOpts { direct_solid_write: true, ..o }),
        ("vein_yrange+top_bound", MatOpts { vein_yrange: true, vein_top_bound: true, ..o }),
        ("cave_top_column_memo", MatOpts { cave_top_column_memo: true, ..o }),
        ("earlier", EARLIER_PATHS),
        ("earlier+band_kernels", MatOpts { band_kernels: true, ..EARLIER_PATHS }),
        ("all but row_kernel", MatOpts { row_kernel: false, ..n }),
        ("all but direct_solid_write", MatOpts { direct_solid_write: false, ..n }),
        ("all but band_kernels", MatOpts { band_kernels: false, row_kernel: false, ..n }),
        ("all", n),
    ]
}

/// Variants of the band kernel tests (each against the old path).
fn kernel_variants() -> Vec<(&'static str, MatOpts)> {
    let o = MatOpts::OLD;
    vec![
        ("band_kernels", MatOpts { band_kernels: true, ..o }),
        ("band_kernels+row_kernel", MatOpts { band_kernels: true, row_kernel: true, ..o }),
        (
            "band_kernels+row_kernel+direct",
            MatOpts { band_kernels: true, row_kernel: true, direct_solid_write: true, ..o },
        ),
        ("all", MatOpts::NEW),
    ]
}

fn generator(pack: &DataPack, preset: &str, dim: &str) -> TerrainGenerator {
    TerrainGenerator::from_preset(pack, preset, dim, SEED).expect("generator")
}

/// Chunks of `t` whose biomes include one of the `names`, on a coarse grid.
fn chunks_with(t: &TerrainGenerator, names: &[&str], n: usize) -> Vec<(i32, i32)> {
    let ids: Vec<u16> =
        names.iter().filter_map(|b| t.biome_names.iter().position(|x| x == b)).map(|i| i as u16).collect();
    assert!(!ids.is_empty(), "biomes {names:?} missing");
    let grid: Vec<(i32, i32)> = (-40..40).flat_map(|x| (-40..40).map(move |z| (x * 9, z * 9))).collect();
    let hits: Vec<bool> =
        grid.par_iter().map(|&(x, z)| t.chunk_biomes(x, z).data.iter().any(|b| ids.contains(b))).collect();
    let found: Vec<(i32, i32)> = grid.iter().zip(&hits).filter(|(_, h)| **h).map(|(c, _)| *c).take(n).collect();
    assert_eq!(found.len(), n, "not enough chunks {names:?}");
    found
}

/// The 64 chunks: (preset, dimension, list).
/// (preset, dimension, chunks).
type Case = (&'static str, &'static str, Vec<(i32, i32)>);

fn cases(pack: &DataPack) -> Vec<Case> {
    let mut rng = Mix(SEED as u64);
    let mut over = vec![(0, 0), (-1, -1), (5, -7), (12, 3), (-200, 140), (333, -91)];
    over.extend((0..10).map(|_| (rng.range(-4000, 4000), rng.range(-4000, 4000))));
    let ow = generator(pack, "minecraft:normal", "minecraft:overworld");
    let bad = chunks_with(&ow, &["minecraft:badlands", "minecraft:eroded_badlands", "minecraft:wooded_badlands"], 10);
    let ice = chunks_with(&ow, &["minecraft:frozen_ocean", "minecraft:deep_frozen_ocean"], 10);
    over.extend(bad);
    over.extend(ice);
    let mut amp = vec![(0, 0), (3, 3)];
    amp.extend((0..8).map(|_| (rng.range(-2000, 2000), rng.range(-2000, 2000))));
    let mut neth = vec![(0, 0), (-2, 5)];
    neth.extend((0..8).map(|_| (rng.range(-1000, 1000), rng.range(-1000, 1000))));
    let mut end = vec![(0, 0), (100, 0), (-80, 60)];
    end.extend((0..5).map(|_| (rng.range(-1000, 1000), rng.range(-1000, 1000))));
    let v = vec![
        ("minecraft:normal", "minecraft:overworld", over),
        ("minecraft:amplified", "minecraft:overworld", amp),
        ("minecraft:normal", "minecraft:the_nether", neth),
        ("minecraft:normal", "minecraft:the_end", end),
    ];
    assert_eq!(v.iter().map(|c| c.2.len()).sum::<usize>(), 64);
    v
}

/// Difference between two chunks, `None` if identical.
fn diff(a: &ProtoTerrain, b: &ProtoTerrain) -> Option<String> {
    if let Some(i) = a.blocks.iter().zip(&b.blocks).position(|(x, y)| x != y) {
        return Some(format!("block {i}: {} ≠ {}", a.blocks[i], b.blocks[i]));
    }
    if a.world_surface_wg != b.world_surface_wg {
        return Some("surface map".into());
    }
    if a.ocean_floor_wg != b.ocean_floor_wg {
        return Some("floor map".into());
    }
    if a.post_processing != b.post_processing {
        return Some(format!("marks ({} against {})", a.post_processing.len(), b.post_processing.len()));
    }
    None
}

fn feed(h: &mut ring::digest::Context, t: &TerrainGenerator, c: &ProtoTerrain) {
    for &b in &c.blocks {
        h.update(t.blocks.name(b).as_bytes());
        h.update(b";");
    }
    for v in c.world_surface_wg.iter().chain(&c.ocean_floor_wg) {
        h.update(&v.to_le_bytes());
    }
    for v in &c.post_processing {
        h.update(&v.to_le_bytes());
    }
}

/// Chunks (surface, then caves) of each variant compared with the old path; fingerprint of
/// the old path. `sabotage` alters a block of a variant (the test must see it).
fn check_chunks(pack: &DataPack, sabotage: bool) -> (Vec<String>, String) {
    let mut failures = Vec::new();
    let mut h = ring::digest::Context::new(&ring::digest::SHA256);
    for (preset, dim, targets) in cases(pack) {
        let mut t = generator(pack, preset, dim);
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &targets);
        t.material.opts = MatOpts::OLD;
        let run = |t: &TerrainGenerator| -> Vec<(ProtoTerrain, ProtoTerrain)> {
            targets
                .par_iter()
                .map(|&(x, z)| (t.generate_until(x, z, &cache, None, Stage::Surface), t.generate(x, z, &cache, None)))
                .collect()
        };
        let reference = run(&t);
        for (s, c) in &reference {
            feed(&mut h, &t, s);
            feed(&mut h, &t, c);
        }
        for (name, o) in variants() {
            t.material.opts = o;
            let got = run(&t);
            for (i, ((rs, rc), (gs, gc))) in reference.iter().zip(&got).enumerate() {
                let mut gs = gs.clone();
                if sabotage && i == 0 {
                    gs.blocks[300] ^= 1;
                }
                for (stage, a, b) in [("surface", rs, &gs), ("caves", rc, gc)] {
                    if let Some(d) = diff(a, b) {
                        failures.push(format!("{dim} {preset} {:?} [{name}] {stage} : {d}", targets[i]));
                    }
                }
            }
        }
    }
    let hex: String = h.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect();
    (failures, hex)
}

#[test]
fn chunks_match_old_path() {
    let Some(pack) = common::data_pack() else { return };
    let (failures, hex) = check_chunks(&pack, false);
    for f in failures.iter().take(20) {
        eprintln!("{f}");
    }
    let mut per: Vec<(&str, usize)> = Vec::new();
    for f in &failures {
        let v = f.split('[').nth(1).and_then(|s| s.split(']').next()).unwrap_or("?");
        match per.iter_mut().find(|p| p.0 == v) {
            Some(p) => p.1 += 1,
            None => per.push((v, 1)),
        }
    }
    eprintln!("differences per variant: {per:?}");
    assert!(failures.is_empty(), "{} differences", failures.len());
    eprintln!("fingerprint {hex}");
    assert_eq!(hex, EXPECTED, "fingerprint of the old path changed");
}

#[test]
fn sabotage_is_seen() {
    let Some(pack) = common::data_pack() else { return };
    let (failures, _) = check_chunks(&pack, true);
    assert!(!failures.is_empty(), "sabotage not seen");
}

/// The overworld veins do have a proven threshold (otherwise the tracks are useless), and
/// the band above the veins, under the preliminary surface, reduces without the vein.
#[test]
fn overworld_gates_found() {
    let Some(pack) = common::data_pack() else { return };
    let t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let gates = t.material.ore_gates();
    assert_eq!(gates.len(), 2);
    let mut ranges: Vec<(f32, f32, f32)> = gates.iter().map(|g| g.expect("proven threshold")).collect();
    ranges.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert_eq!(ranges, [(-60.0, -8.0, -1.0), (0.0, 50.0, -1.0)]);
    let bands = t.material.band_roots();
    // Expected bounds: bedrock floor (0..5 above the bottom), iron, deepslate, copper.
    let cuts: Vec<i32> = bands.iter().map(|b| b.0).collect();
    for y in [-64, -59, -60, -8, 0, 8, 50] {
        assert!(cuts.contains(&y), "bound {y} missing from {cuts:?}");
    }
}

/// Modified pack: positive out-of-range constant for copper (threshold refused), pure
/// height condition added (band), old and new path identical.
#[test]
fn modified_pack_matches() {
    let Some(pack) = common::data_pack() else { return };
    let mut p = pack.clone();
    let key = "density_function/minecraft:overworld/ore_vein/copper_density";
    let cu = p.raw("density_function", "minecraft:overworld/ore_vein/copper_density").expect("copper").to_owned();
    let i = cu.rfind("\"when_out_of_range\": -1.0").expect("out-of-range constant");
    // Out of range: a noise (density variable everywhere, veins also read by the caves).
    let noise =
        r#""when_out_of_range": {"type":"minecraft:noise","noise":"minecraft:ore_gap","xz_scale":1.0,"y_scale":1.0}"#;
    let cu = format!("{}{noise}{}", &cu[..i], &cu[i + "\"when_out_of_range\": -1.0".len()..]);
    p.insert(key, cu);
    let under = r#"{"type":"minecraft:sequence","sequence":[
        {"type":"minecraft:condition","if_true":{"type":"minecraft:y_above","anchor":{"absolute":30},
         "surface_depth_multiplier":0,"add_stone_depth":false},
         "then_run":{"type":"minecraft:block","result_state":{"Name":"minecraft:tuff"}}},
        {"type":"minecraft:condition","if_true":{"type":"minecraft:vertical_gradient","random_name":"minecraft:deepslate",
         "true_at_and_below":{"absolute":0},"false_at_and_above":{"absolute":8}},
         "then_run":{"type":"minecraft:block","result_state":{"Name":"minecraft:deepslate"}}}]}"#;
    p.insert("material_rule/minecraft:overworld/underground", under.to_owned());
    // Temperature condition (absent from the 26.3 game rules): above the snow limit
    // and in frozen oceans, to cover the altitude correction and the frozen part.
    let root = r#"{"type":"minecraft:sequence","sequence":["minecraft:bedrock_floor",
        "minecraft:overworld/copper_ore_vein","minecraft:overworld/iron_ore_vein",
        {"type":"minecraft:condition","if_true":{"type":"minecraft:y_above","anchor":{"absolute":81},
         "surface_depth_multiplier":0,"add_stone_depth":false},
         "then_run":{"type":"minecraft:condition","if_true":{"type":"minecraft:temperature"},
          "then_run":{"type":"minecraft:block","result_state":{"Name":"minecraft:blue_ice"}}}},
        {"type":"minecraft:condition","if_true":{"type":"minecraft:biome",
          "biome_is":["minecraft:frozen_ocean","minecraft:deep_frozen_ocean"]},
         "then_run":{"type":"minecraft:condition","if_true":{"type":"minecraft:temperature"},
          "then_run":{"type":"minecraft:block","result_state":{"Name":"minecraft:packed_ice"}}}},
        {"type":"minecraft:condition","if_true":{"type":"minecraft:above_preliminary_surface"},
         "then_run":"minecraft:overworld/surface"},
        "minecraft:overworld/underground"]}"#;
    p.insert("material_rule/minecraft:overworld", root.to_owned());
    let mut t = generator(&p, "minecraft:normal", "minecraft:overworld");
    let gates = t.material.ore_gates();
    assert!(gates.iter().filter(|g| g.is_none()).count() == 1, "modified copper threshold accepted: {gates:?}");
    assert!(t.material.band_roots().iter().any(|b| b.0 == 30), "bound 30 missing");
    let mut targets = vec![(0, 0), (7, -3), (-40, 22), (150, 150)];
    targets.extend(chunks_with(
        &t,
        &["minecraft:windswept_hills", "minecraft:windswept_gravelly_hills", "minecraft:old_growth_pine_taiga"],
        6,
    ));
    targets.extend(chunks_with(&t, &["minecraft:grove", "minecraft:snowy_slopes", "minecraft:jagged_peaks"], 3));
    targets.extend(chunks_with(&t, &["minecraft:frozen_ocean", "minecraft:deep_frozen_ocean"], 4));
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &targets);
    t.material.opts = MatOpts::OLD;
    let old: Vec<ProtoTerrain> = targets.par_iter().map(|&(x, z)| t.generate(x, z, &cache, None)).collect();
    for b in ["minecraft:tuff", "minecraft:blue_ice", "minecraft:packed_ice"] {
        let id = t.blocks.get(b).expect("block");
        assert!(old.iter().any(|c| c.blocks.contains(&id)), "modified rule without effect ({b})");
    }
    let mut fails = Vec::new();
    for (name, o) in variants() {
        t.material.opts = o;
        let new: Vec<ProtoTerrain> = targets.par_iter().map(|&(x, z)| t.generate(x, z, &cache, None)).collect();
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            if let Some(d) = diff(a, b) {
                fails.push(format!("[{name}] {:?} : {d}", targets[i]));
            }
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Vein buffers restricted to the live rows == full buffers (density everywhere,
/// richness where it is read), over 200 volumes: shrunk chunks (top under 0 and under −8)
/// and volumes of arbitrary y origin.
#[test]
fn restricted_vein_buffers_match() {
    let Some(pack) = common::data_pack() else { return };
    let t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let mut rng = Mix(0xC1_0808);
    let mut vols = Vec::new();
    for sy in [1, 8, 9, 50, 56, 57, 63, 64, 65, 72, 114, 121, 184, 200, 384] {
        vols.push(Volume::new(16, sy, 16, rng.range(-500, 500) * 16, -64, rng.range(-500, 500) * 16));
    }
    while vols.len() < 200 {
        let (sx, sz) = (rng.range(1, 17), rng.range(1, 17));
        vols.push(Volume::new(
            sx,
            rng.range(1, 120),
            sz,
            rng.range(-9000, 9000),
            rng.range(-70, 80),
            rng.range(-9000, 9000),
        ));
    }
    let opts = [
        MatOpts { vein_yrange: true, ..MatOpts::OLD },
        MatOpts { vein_yrange: true, vein_lazyrich: true, ..MatOpts::OLD },
        MatOpts { vein_yrange: true, vein_shared: true, ..MatOpts::OLD },
        MatOpts::NEW,
    ];
    let bufs = |v: Volume, o: MatOpts| {
        let mut ctx = Ctx::cached();
        let mut view = BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() };
        let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
        t.material.prefill_with(&mut env, v, o, None)
    };
    let fails: Vec<String> = vols
        .par_iter()
        .flat_map(|&v| {
            let old = bufs(v, MatOpts::OLD);
            let mut f = Vec::new();
            for o in opts {
                for (k, (a, b)) in old.iter().zip(bufs(v, o)).enumerate() {
                    let ((da, ra), (db, rb)) = (a.as_ref().expect("old"), b.expect("new"));
                    for j in 0..da.len() {
                        let read = da[j] > 0.0 || da[j].is_nan();
                        if da[j].to_bits() != db[j].to_bits() || (read && ra[j].to_bits() != rb[j].to_bits()) {
                            f.push(format!("{v:?} vein {k} {o:?} element {j}"));
                            break;
                        }
                    }
                }
            }
            f
        })
        .collect();
    for f in fails.iter().take(10) {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Index of a block: shift and mask == divisions, 10⁶ (volume, position), step 1 to 16.
#[test]
fn volume_index_pow2_matches() {
    let mut rng = Mix(42);
    let mut hits = 0;
    for _ in 0..1_000_000 {
        let st = [rng.range(1, 17), rng.range(1, 17), rng.range(1, 17)];
        let st = if rng.next().is_multiple_of(2) { st.map(|s| 1 << (s % 5)) } else { st };
        let sz = [rng.range(1, 20), rng.range(1, 20), rng.range(1, 20)];
        let o = [rng.range(-100_000, 100_000), rng.range(-2000, 2000), rng.range(-100_000, 100_000)];
        let v = Volume::stepped(sz[0], sz[1], sz[2], o[0], o[1], o[2], st[0], st[1], st[2]);
        let p: [i32; 3] = std::array::from_fn(|a| {
            if rng.next().is_multiple_of(97) {
                [i32::MIN, i32::MAX, -1, 0][(rng.next() % 4) as usize]
            } else {
                o[a] + rng.range(-2 * st[a], sz[a] * st[a] + 2 * st[a])
            }
        });
        let a = v.index_of_block_with(false, p[0], p[1], p[2]);
        assert_eq!(a, v.index_of_block_with(true, p[0], p[1], p[2]), "{v:?} {p:?}");
        hits += usize::from(a.is_some());
    }
    assert!(hits > 5_000, "too few positions in the volumes ({hits})");
}

/// Top material of the caves (replayed per block): 40,000 successive calls on the same thread
/// (state reused, veins read on demand), y covering the veins, old == each variant.
#[test]
fn top_material_matches() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let mut rng = Mix(808);
    let targets: Vec<(i32, i32)> = (0..24).map(|_| (rng.range(-3000, 3000), rng.range(-3000, 3000))).collect();
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &targets);
    t.material.opts = MatOpts::OLD;
    let chunks: Vec<ProtoTerrain> =
        targets.par_iter().map(|&(x, z)| t.generate_until(x, z, &cache, None, Stage::Surface)).collect();
    // Vein blocks already placed by the surface: their neighbours cover density > 0 and draws.
    let vein_ids: Vec<u16> = ["granite", "tuff", "copper_ore", "raw_copper_block", "iron_ore", "raw_iron_block"]
        .iter()
        .filter_map(|n| t.blocks.get(&format!("minecraft:{n}")))
        .collect();
    let mut calls: Vec<(usize, i32, i32, i32, bool)> = Vec::new();
    for (c, ch) in chunks.iter().enumerate() {
        for (i, b) in ch.blocks.iter().enumerate() {
            if vein_ids.contains(b) && calls.len() < 30_000 && rng.next().is_multiple_of(4) {
                let (x, z, y) = ((i & 15) as i32, ((i >> 4) & 15) as i32, (i >> 8) as i32 + ch.min_y);
                calls.push((c, x, y + rng.range(-1, 2), z, rng.next().is_multiple_of(3)));
            }
        }
    }
    let n_veins = calls.len();
    calls.extend((0..20_000).map(|_| {
        (
            rng.range(0, 24) as usize,
            rng.range(0, 16),
            rng.range(-64, 130),
            rng.range(0, 16),
            rng.next().is_multiple_of(3),
        )
    }));
    // Shuffle (fixed order): neighbouring calls do not follow each other.
    for i in (1..calls.len()).rev() {
        calls.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    eprintln!("{n_veins} calls near the veins, {} in all", calls.len());
    let run = |t: &TerrainGenerator| -> Vec<Option<u16>> {
        let mut ctx = Ctx::cached();
        let mut view = BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() };
        let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
        calls.iter().map(|&(c, x, y, z, f)| t.material.top_material(&chunks[c], &mut env, x, y, z, f)).collect()
    };
    let old = run(&t);
    assert!(old.iter().filter(|b| b.is_some()).count() > 1000, "too few blocks returned");
    let veins = old.iter().filter(|b| b.is_some_and(|b| vein_ids.contains(&b))).count();
    eprintln!("vein blocks returned: {veins}");
    assert!(veins > 100, "veins never reached");
    let mut fails = Vec::new();
    for (name, o) in variants() {
        t.material.opts = o;
        let got = run(&t);
        let bad = old.iter().zip(&got).filter(|(a, b)| a != b).count();
        if bad > 0 {
            fails.push(format!("[{name}] {bad} different calls"));
        }
    }
    assert!(fails.is_empty(), "{fails:?}");
}

/// Temperature condition: path kept per column (frozen part, altitude noise, bound)
/// == direct computation, for each biome, 400 columns and 48 heights per column.
#[test]
fn cold_matches_temperature() {
    let Some(pack) = common::data_pack() else { return };
    let t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let n = t.biome_names.len() as u16;
    let fails: Vec<String> = (0..n)
        .into_par_iter()
        .flat_map(|b| {
            let mut rng = Mix(u64::from(b) + 99);
            let mut f = Vec::new();
            for _ in 0..400 {
                let (x, z) = (rng.range(-3_000_000, 3_000_000), rng.range(-3_000_000, 3_000_000));
                let ys: Vec<i32> = (0..48).map(|_| rng.range(40, 330)).collect();
                for (y, (a, c)) in ys.iter().zip(t.material.cold_both(b, x, z, &ys)) {
                    if a != c {
                        f.push(format!("{} ({x}, {y}, {z}): {a} against {c}", t.biome_names[b as usize]));
                    }
                }
            }
            f
        })
        .collect();
    for f in fails.iter().take(10) {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Bound of the temperature noise used to discard the computation (|v| ≤ bound).
#[test]
fn temperature_noise_bound() {
    let n = Simplex::new_without_offset(&mut Rng::legacy(1234));
    let mut rng = Mix(7);
    let mut max = 0.0f32;
    for _ in 0..2_000_000 {
        let (x, z) = (rng.range(-30_000_000, 30_000_000), rng.range(-30_000_000, 30_000_000));
        let v = n.get2(f64::from(x as f32 / 8.0), f64::from(z as f32 / 8.0)).abs();
        max = max.max(v);
    }
    for i in 0..1_000_000 {
        let (x, z) = (f64::from(i % 1000) * 0.013, f64::from(i / 1000) * 0.017);
        max = max.max(n.get2(x, z).abs());
    }
    eprintln!("|temperature noise| max {max}");
    assert!(max <= TEMPERATURE_NOISE_BOUND, "bound exceeded: {max}");
}

/// Up to `n` chunks of `t` whose biomes include one of `names`, on a grid of `step` chunks.
fn some_chunks_with(t: &TerrainGenerator, names: &[&str], n: usize, step: i32) -> Vec<(i32, i32)> {
    let ids: Vec<u16> =
        names.iter().filter_map(|b| t.biome_names.iter().position(|x| x == b)).map(|i| i as u16).collect();
    let grid: Vec<(i32, i32)> = (-30..30).flat_map(|x| (-30..30).map(move |z| (x * step, z * step))).collect();
    let hits: Vec<bool> =
        grid.par_iter().map(|&(x, z)| t.chunk_biomes(x, z).data.iter().any(|b| ids.contains(b))).collect();
    grid.iter().zip(&hits).filter(|(_, h)| **h).map(|(c, _)| *c).take(n).collect()
}

/// Biomes of the 3 × 3 chunks around (cx, cz), in the order of the surface step.
fn region(cache: &BiomeCache, cx: i32, cz: i32) -> [&ChunkBiomes; 9] {
    std::array::from_fn(|i| &cache.chunks[&(cx + i as i32 / 3 - 1, cz + i as i32 % 3 - 1)])
}

/// Chunks (surface step, then caves) of each variant against the old path: the differences.
fn compare_with_old(
    t: &mut TerrainGenerator,
    cache: &BiomeCache,
    targets: &[(i32, i32)],
    variants: &[(&str, MatOpts)],
    label: &str,
) -> Vec<String> {
    let run = |t: &TerrainGenerator| -> Vec<(ProtoTerrain, ProtoTerrain)> {
        targets
            .par_iter()
            .map(|&(x, z)| (t.generate_until(x, z, cache, None, Stage::Surface), t.generate(x, z, cache, None)))
            .collect()
    };
    t.material.opts = MatOpts::OLD;
    let reference = run(t);
    let mut fails = Vec::new();
    for (name, o) in variants {
        t.material.opts = *o;
        for (i, ((rs, rc), (gs, gc))) in reference.iter().zip(run(t)).enumerate() {
            for (stage, a, b) in [("surface", rs, &gs), ("caves", rc, &gc)] {
                if let Some(d) = diff(a, b) {
                    fails.push(format!("{label} {:?} [{name}] {stage}: {d}", targets[i]));
                }
            }
        }
    }
    t.material.opts = MatOpts::OLD;
    fails
}

/// Kernels found for the 26.3 overworld under the preliminary surface (the equivalence tests would
/// prove nothing if the walk silently kept every band), none for the nether and the end.
#[test]
fn vanilla_band_kernels() {
    let Some(pack) = common::data_pack() else { return };
    let t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let k = t.material.below_surface_kernels().expect("kernels under the preliminary surface");
    let id = |n: &str| t.blocks.get(n).unwrap_or_else(|| panic!("{n}"));
    let at = |y: i32| *k.iter().find(|b| b.0 <= y && y < b.1).unwrap_or_else(|| panic!("no band at {y}"));
    let (bedrock, deepslate) = (id("minecraft:bedrock"), id("minecraft:deepslate"));
    let (copper, iron) = (0, 1);
    // Without the sulfur caves condition: a block, veins, nothing.
    assert_eq!(at(-64).3, BandKernel::Block(bedrock));
    assert!(matches!(at(-62).3, BandKernel::Tree(_)), "bedrock gradient");
    assert_eq!(at(-30).3, BandKernel::VeinOrBlock(iron, deepslate));
    assert_eq!(at(-4).3, BandKernel::Block(deepslate));
    assert_eq!(at(0).3, BandKernel::VeinOrBlock(copper, deepslate));
    assert!(matches!(at(4).3, BandKernel::Tree(_)), "deepslate gradient");
    assert_eq!(at(20).3, BandKernel::Vein(copper));
    assert_eq!(at(60).3, BandKernel::Nothing);
    // With it, the cells read their biome.
    assert!(matches!(at(-30).2, BandKernel::Tree(_)));
    assert!(matches!(at(60).2, BandKernel::Tree(_)));
    for dim in ["minecraft:the_nether", "minecraft:the_end"] {
        assert!(generator(&pack, "minecraft:normal", dim).material.below_surface_kernels().is_none(), "{dim}");
    }
}

/// Band kernels against the old path: 4 settings × 3 seeds × 10 chunks (random, peaks with cliffs
/// and overhangs, deep oceans, sulfur caves) and 24 badlands chunks, blocks, maps and marks.
#[test]
fn band_kernels_across_settings() {
    let Some(pack) = common::data_pack() else { return };
    let settings = [
        ("minecraft:normal", "minecraft:overworld"),
        ("minecraft:amplified", "minecraft:overworld"),
        ("minecraft:large_biomes", "minecraft:overworld"),
        ("minecraft:normal", "minecraft:the_nether"),
    ];
    let mut fails = Vec::new();
    let (mut with_sulfur, mut without_sulfur) = (0, 0);
    for (preset, dim) in settings {
        for seed in [SEED, 1, -7_331_000_123] {
            let mut t = TerrainGenerator::from_preset(&pack, preset, dim, seed).expect("generator");
            let mut rng = Mix(seed as u64 ^ 0xB4D5);
            let mut targets: Vec<(i32, i32)> =
                (0..4).map(|_| (rng.range(-3000, 3000), rng.range(-3000, 3000))).collect();
            if dim == "minecraft:overworld" {
                let peaks = ["minecraft:jagged_peaks", "minecraft:stony_peaks", "minecraft:windswept_hills"];
                targets.extend(some_chunks_with(&t, &peaks, 2, 7));
                let deep = ["minecraft:deep_ocean", "minecraft:deep_cold_ocean", "minecraft:deep_frozen_ocean"];
                targets.extend(some_chunks_with(&t, &deep, 2, 7));
                targets.extend(some_chunks_with(&t, &["minecraft:sulfur_caves"], 2, 5));
            }
            while targets.len() < 10 {
                targets.push((rng.range(-3000, 3000), rng.range(-3000, 3000)));
            }
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &targets);
            if let Some(s) = t.biome_names.iter().position(|b| b == "minecraft:sulfur_caves") {
                for &(x, z) in &targets {
                    match possible_biomes(t.biome_names.len(), &region(&cache, x, z))[s] {
                        true => with_sulfur += 1,
                        false => without_sulfur += 1,
                    }
                }
            }
            fails.extend(compare_with_old(
                &mut t,
                &cache,
                &targets,
                &kernel_variants(),
                &format!("{preset} {dim} {seed}"),
            ));
        }
    }
    let mut t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let bad = chunks_with(&t, &["minecraft:badlands", "minecraft:eroded_badlands", "minecraft:wooded_badlands"], 24);
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &bad);
    fails.extend(compare_with_old(&mut t, &cache, &bad, &kernel_variants(), "badlands"));
    for f in fails.iter().take(20) {
        eprintln!("{f}");
    }
    eprintln!("overworld chunks with sulfur caves around: {with_sulfur}, without: {without_sulfur}");
    assert!(with_sulfur > 0 && without_sulfur > 0, "both kernel sets must be exercised");
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Modified packs: stone depth read under the surface (no kernels), preliminary surface forced
/// under the bands, inside the deepslate gradient and above everything, fluid and air placed
/// under the surface (by a block band and by a vein filler); old path == kernels.
#[test]
fn band_kernels_synthetic_packs() {
    let Some(pack) = common::data_pack() else { return };
    let underground = "material_rule/minecraft:overworld/underground";
    let level = "density_function/minecraft:overworld/chunk_surface_level";
    let copper_key = "material_rule/minecraft:overworld/copper_ore_vein";
    let copper = pack.raw("material_rule", "minecraft:overworld/copper_ore_vein").expect("copper vein").to_owned();
    let stone_depth = r#"{"type":"minecraft:sequence","sequence":[
        {"type":"minecraft:condition","if_true":{"type":"minecraft:stone_depth","offset":2,
          "add_surface_depth":false,"secondary_depth_range":0,"surface_type":"floor"},
         "then_run":{"type":"minecraft:block","result_state":{"Name":"minecraft:tuff"}}},
        {"type":"minecraft:block","result_state":{"Name":"minecraft:deepslate"}}]}"#;
    let fluid_and_air = r#"{"type":"minecraft:sequence","sequence":[
        {"type":"minecraft:condition","if_true":{"type":"minecraft:y_above","anchor":{"absolute":-20},
          "surface_depth_multiplier":0,"add_stone_depth":false},
         "then_run":{"type":"minecraft:block","result_state":{"Name":"minecraft:water"}}},
        {"type":"minecraft:block","result_state":{"Name":"minecraft:cave_air"}}]}"#;
    let water_filler = copper.replace("minecraft:granite", "minecraft:water");
    assert_ne!(water_filler, copper, "copper filler not found");
    // (label, files replaced, kernels expected)
    type PackCase = (&'static str, Vec<(&'static str, String)>, bool);
    let cases: Vec<PackCase> = vec![
        ("stone depth under the surface", vec![(underground, stone_depth.to_owned())], false),
        ("preliminary surface at -30", vec![(level, "-30.0".to_owned())], true),
        ("preliminary surface at 4", vec![(level, "4.0".to_owned())], true),
        ("preliminary surface at 400", vec![(level, "400.0".to_owned())], true),
        (
            "fluid and air under the surface",
            vec![(underground, fluid_and_air.to_owned()), (copper_key, water_filler)],
            true,
        ),
    ];
    let mut fails = Vec::new();
    for (label, files, kernels) in cases {
        let mut p = pack.clone();
        for (key, text) in files {
            p.insert(key, text);
        }
        let mut t = generator(&p, "minecraft:normal", "minecraft:overworld");
        assert_eq!(t.material.below_surface_kernels().is_some(), kernels, "{label}: kernels");
        let mut targets = vec![(0, 0), (7, -3), (-40, 22), (150, 150)];
        targets.extend(some_chunks_with(&t, &["minecraft:jagged_peaks", "minecraft:windswept_hills"], 2, 7));
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &targets);
        fails.extend(compare_with_old(&mut t, &cache, &targets, &kernel_variants(), label));
    }
    for f in fails.iter().take(20) {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Surface step of filled chunks where other blocks were planted under the surface (a plain solid
/// other than the default block, solids that do not block motion, a waterlogged one, cave air): the
/// in-place writes and the layer kernel take their slow paths there; old path == kernels.
#[test]
fn band_kernels_on_planted_blocks() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let planted: Vec<u16> = [
        "minecraft:tuff",
        "minecraft:oak_sapling",
        "minecraft:oak_leaves[waterlogged=true]",
        "minecraft:cave_air",
        "minecraft:oak_log[axis=x]",
        "minecraft:torch",
    ]
    .iter()
    .map(|s| t.blocks.intern(s))
    .collect();
    let b = &t.blocks;
    assert!(b.is_plain_solid(planted[0]) && planted[0] != t.material.default_block, "plain solid");
    assert!(b.is_solid(planted[1]) && !b.blocks_motion(planted[1]), "solid that does not block motion");
    assert!(b.has_fluid(planted[2]), "waterlogged");
    let mut targets = vec![(0, 0), (3, 9), (-20, 11), (77, -5)];
    targets.extend(some_chunks_with(&t, &["minecraft:jagged_peaks", "minecraft:windswept_hills"], 2, 7));
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &targets);
    let mut rng = Mix(0x91A47);
    let filled: Vec<ProtoTerrain> = targets
        .iter()
        .map(|&(cx, cz)| {
            let mut c = t.generate_until(cx, cz, &cache, None, Stage::Fill);
            for _ in 0..800 {
                let (x, y, z) = (rng.range(0, 16), rng.range(-64, 40), rng.range(0, 16));
                c.set(&t.blocks, x, y, z, planted[rng.range(0, planted.len() as i32) as usize]);
            }
            c
        })
        .collect();
    let surface = |t: &TerrainGenerator, filled: &ProtoTerrain| -> ProtoTerrain {
        let mut c = filled.clone();
        let chunks = region(&cache, c.cx, c.cz);
        let possible = possible_biomes(t.biome_names.len(), &chunks);
        let (min_qy, max_qy) = (t.world_min_y >> 2, (t.world_min_y >> 2) + (t.world_height >> 2) - 1);
        let mut view = BiomeView::Region { chunks, cx: c.cx, cz: c.cz, zoom_seed: t.zoom_seed, min_qy, max_qy };
        let mut ctx = Ctx::cached();
        let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
        let full = t.fill_volume(c.cx, c.cz).expect("filling volume");
        t.material.build_surface(&mut c, &t.blocks, &mut env, full, Some(&possible));
        c
    };
    t.material.opts = MatOpts::OLD;
    let old: Vec<ProtoTerrain> = filled.par_iter().map(|c| surface(&t, c)).collect();
    let mut fails = Vec::new();
    for (name, o) in kernel_variants() {
        t.material.opts = o;
        let new: Vec<ProtoTerrain> = filled.par_iter().map(|c| surface(&t, c)).collect();
        for (i, (a, b)) in old.iter().zip(&new).enumerate() {
            if let Some(d) = diff(a, b) {
                fails.push(format!("{:?} [{name}]: {d}", targets[i]));
            }
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Vein pre-filling bounded by the highest solid: same density (and richness where read) on
/// the rows up to the bound, 120 volumes with random bounds; chunks of deep and frozen oceans and
/// eroded badlands identical to the old path.
#[test]
fn vein_top_bound_matches() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    let mut rng = Mix(0x70B0);
    let vols: Vec<(Volume, i32)> = (0..120)
        .map(|_| {
            let v = Volume::new(16, rng.range(40, 200), 16, rng.range(-500, 500) * 16, -64, rng.range(-500, 500) * 16);
            (v, rng.range(-70, 140))
        })
        .collect();
    let bounded = MatOpts { vein_yrange: true, vein_top_bound: true, ..MatOpts::OLD };
    let fails: Vec<String> = vols
        .par_iter()
        .flat_map(|&(v, top)| {
            let mut ctx = Ctx::cached();
            let mut view = BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() };
            let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
            let old = t.material.prefill_with(&mut env, v, MatOpts::OLD, None);
            let mut f = Vec::new();
            for o in [bounded, MatOpts::NEW] {
                for (k, (a, b)) in old.iter().zip(t.material.prefill_with(&mut env, v, o, Some(top))).enumerate() {
                    let ((da, ra), (db, rb)) = (a.as_ref().expect("old"), b.expect("new"));
                    for j in 0..da.len() {
                        if v.by(j as i32 % v.sy) > top {
                            continue;
                        }
                        let read = da[j] > 0.0 || da[j].is_nan();
                        if da[j].to_bits() != db[j].to_bits() || (read && ra[j].to_bits() != rb[j].to_bits()) {
                            f.push(format!("{v:?} top {top} vein {k} element {j}"));
                            break;
                        }
                    }
                }
            }
            f
        })
        .collect();
    let mut targets = some_chunks_with(&t, &["minecraft:deep_ocean", "minecraft:deep_cold_ocean"], 6, 9);
    targets.extend(chunks_with(&t, &["minecraft:frozen_ocean", "minecraft:deep_frozen_ocean"], 4));
    targets.extend(chunks_with(&t, &["minecraft:eroded_badlands"], 4));
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &targets);
    let variants = [("vein_yrange+top_bound", bounded), ("all", MatOpts::NEW)];
    let chunk_fails = compare_with_old(&mut t, &cache, &targets, &variants, "oceans and badlands");
    for f in fails.iter().chain(&chunk_fails).take(20) {
        eprintln!("{f}");
    }
    assert!(
        fails.is_empty() && chunk_fails.is_empty(),
        "{} buffer and {} chunk differences",
        fails.len(),
        chunk_fails.len()
    );
}

/// Vein bound with a copper density of 0.7 to 0.9 over its whole range: every solid cell read in
/// [0, 50[ draws the vein, so a bound one row too low changes the top cell of the deep ocean
/// floors (the vanilla veins are too sparse for that sabotage to show). A constant density would
/// be folded and lose its y threshold, hence the noise.
#[test]
fn vein_top_bound_dense_copper() {
    let Some(pack) = common::data_pack() else { return };
    let mut p = pack.clone();
    let dense = r#"{"type":"minecraft:range_choice","input":"minecraft:y","min_inclusive":0.0,"max_exclusive":50.0,
        "when_in_range":{"type":"minecraft:add","left":0.8,
          "right":{"type":"minecraft:mul","left":0.1,"right":"minecraft:overworld/ore_vein/toggle"}},
        "when_out_of_range":-1.0}"#;
    p.insert("density_function/minecraft:overworld/ore_vein/copper_density", dense.to_owned());
    let mut t = generator(&p, "minecraft:normal", "minecraft:overworld");
    assert!(t.material.ore_gates().iter().all(Option::is_some), "dense copper without its y threshold");
    let mut targets = some_chunks_with(
        &t,
        &["minecraft:deep_ocean", "minecraft:deep_cold_ocean", "minecraft:deep_lukewarm_ocean"],
        8,
        9,
    );
    targets.extend(some_chunks_with(&t, &["minecraft:frozen_ocean", "minecraft:deep_frozen_ocean"], 3, 9));
    assert!(targets.len() >= 8, "too few ocean chunks");
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &targets);
    // The bound only acts on chunks whose highest solid is under the copper range's top.
    let tops: Vec<i32> = targets
        .par_iter()
        .map(|&(x, z)| {
            let c = t.generate_until(x, z, &cache, None, Stage::Fill);
            (0..256)
                .map(|i| {
                    let (x, z) = (i & 15, i >> 4);
                    (c.min_y..=c.surface_height(x, z))
                        .rev()
                        .find(|&y| t.blocks.is_solid(c.get(x, y, z)))
                        .unwrap_or(c.min_y)
                })
                .max()
                .unwrap_or(c.min_y)
        })
        .collect();
    eprintln!("highest solids: {tops:?}");
    assert!(tops.iter().filter(|t| **t < 50).count() >= 3, "too few chunks under the copper range's top");
    let bounded = MatOpts { vein_yrange: true, vein_top_bound: true, ..MatOpts::OLD };
    let fails = compare_with_old(
        &mut t,
        &cache,
        &targets,
        &[("vein_yrange+top_bound", bounded), ("all", MatOpts::NEW)],
        "dense copper",
    );
    for f in fails.iter().take(20) {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Biome seen by the cave top material at each real call (500 chunks × 3 seeds × 3 settings):
/// the chunks around give what the source gives, except where the climate is tied between
/// biomes (the last leaf decides: counted, not compared). Built calls at the world's bottom and
/// top: the source is asked outside the height, never the chunks (no clamp, routing checked).
#[test]
fn cave_top_biomes_from_chunks() {
    /// Biome no source returns: tells which way the view read.
    const MARKER: u16 = u16::MAX;
    let Some(pack) = common::data_pack() else { return };
    let (mut calls_seen, mut tied, mut outside_seen, mut fails) = (0usize, 0usize, 0usize, Vec::new());
    for preset in ["minecraft:normal", "minecraft:amplified", "minecraft:large_biomes"] {
        for seed in [SEED, 2, -90_210] {
            let t = TerrainGenerator::from_preset(&pack, preset, "minecraft:overworld", seed).expect("generator");
            let mut rng = Mix(seed as u64 ^ 0xCA7E);
            let targets: Vec<(i32, i32)> = (0..500).map(|_| (rng.range(-5000, 5000), rng.range(-5000, 5000))).collect();
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &targets);
            let BiomeSource::MultiNoise(list) = &t.source else { panic!("multi-noise source expected") };
            let (min_qy, max_qy) = (t.world_min_y >> 2, (t.world_min_y >> 2) + (t.world_height >> 2) - 1);
            let height_q = max_qy - min_qy + 1;
            let marker = ChunkBiomes { min_qy, height_q, data: vec![MARKER; (height_q * 16) as usize] };
            let results: Vec<(usize, usize, usize, Vec<String>)> = targets
                .par_iter()
                .enumerate()
                .map(|(n, &(cx, cz))| {
                    let (_, calls) = record_top_material_calls(|| t.generate(cx, cz, &cache, None));
                    // Built calls at the bottom and top of the world, every column of 40 chunks.
                    let mut built: Vec<[i32; 3]> = Vec::new();
                    if n < 40 {
                        for i in 0..256 {
                            let (x, z) = (cx * 16 + i % 16, cz * 16 + i / 16);
                            built.extend([-64, -63, 318, 319].map(|y| [x, y, z]));
                        }
                    }
                    let view_of_chunks = || BiomeView::ChunkThenSource {
                        chunks: region(&cache, cx, cz),
                        cx,
                        cz,
                        zoom_seed: t.zoom_seed,
                        min_qy,
                        max_qy,
                        src: &t.source,
                        searcher: Searcher::default(),
                    };
                    let source =
                        || BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() };
                    let tied_at = |ctx: &mut Ctx, (qx, qy, qz): (i32, i32, i32)| {
                        list.tied_biomes(BiomeSource::climate_at(&t.g, ctx, qx, qy, qz)) > 1
                    };
                    let (mut tied, mut outside_calls, mut fails) = (0, 0, Vec::new());
                    let mut ctx = Ctx::cached();
                    // Real calls in their order, one last leaf per chunk like the caves step.
                    let (mut old_view, mut chunk_view) = (source(), view_of_chunks());
                    for &[x, y, z] in &calls {
                        let (expected, got) = (old_view.get(&t.g, x, y, z), chunk_view.get(&t.g, x, y, z));
                        if got != expected {
                            if tied_at(&mut ctx, zoom_cached(t.zoom_seed, x, y, z)) {
                                tied += 1;
                            } else {
                                fails.push(format!("{preset} {seed} ({x}, {y}, {z}): {got} against {expected}"));
                            }
                        }
                    }
                    // Built calls with fresh views: outside the height both ask the source from the
                    // same (empty) last leaf, so they must agree even on a tie. Routing checked on
                    // chunks holding an impossible biome: read inside the height, never outside
                    // (a clamp would read them there; the real biomes rarely tell, the quarts
                    // just inside give the same biome).
                    let marker_view = || BiomeView::ChunkThenSource {
                        chunks: [&marker; 9],
                        cx,
                        cz,
                        zoom_seed: t.zoom_seed,
                        min_qy,
                        max_qy,
                        src: &t.source,
                        searcher: Searcher::default(),
                    };
                    for &[x, y, z] in &built {
                        let q = zoom_cached(t.zoom_seed, x, y, z);
                        let outside = !(min_qy..=max_qy).contains(&q.1);
                        let expected = source().get(&t.g, x, y, z);
                        let got = view_of_chunks().get(&t.g, x, y, z);
                        let routed = marker_view().get(&t.g, x, y, z);
                        outside_calls += usize::from(outside);
                        if (outside && routed != expected) || (!outside && routed != MARKER) {
                            fails.push(format!("{preset} {seed} built ({x}, {y}, {z}): routed to {routed}"));
                        }
                        if got != expected {
                            if !outside && tied_at(&mut ctx, q) {
                                tied += 1;
                            } else {
                                fails.push(format!("{preset} {seed} built ({x}, {y}, {z}): {got} against {expected}"));
                            }
                        }
                    }
                    (calls.len(), tied, outside_calls, fails)
                })
                .collect();
            for (n, ti, out, f) in results {
                calls_seen += n;
                tied += ti;
                outside_seen += out;
                fails.extend(f);
            }
        }
    }
    for f in fails.iter().take(20) {
        eprintln!("{f}");
    }
    eprintln!("{calls_seen} real calls, {tied} tied climates, {outside_seen} built calls outside the height");
    assert!(calls_seen > 10_000, "too few cave top material calls ({calls_seen})");
    assert!(outside_seen > 1000, "too few built calls outside the height ({outside_seen})");
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Column levels kept by the surface step == those the cave top material computes without them
/// (one-block volume), over the 256 columns of 200 chunks; the memo is found on the same thread.
#[test]
fn cave_top_column_memo_matches() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = generator(&pack, "minecraft:normal", "minecraft:overworld");
    t.material.opts = MatOpts::NEW;
    let mut rng = Mix(0x3E30);
    let targets: Vec<(i32, i32)> = (0..200).map(|_| (rng.range(-4000, 4000), rng.range(-4000, 4000))).collect();
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &targets);
    let fails: Vec<String> = targets
        .par_iter()
        .flat_map(|&(cx, cz)| {
            let chunk = t.generate_until(cx, cz, &cache, None, Stage::Surface);
            let mut ctx = Ctx::cached();
            let mut view = BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() };
            let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
            let mut f = Vec::new();
            for i in 0..256 {
                let (x, z) = (i & 15, i >> 4);
                let (computed, kept) = t.material.cave_column_levels(&mut env, &chunk, x, z);
                if kept != Some(computed) {
                    f.push(format!("({cx}, {cz}) column ({x}, {z}): {computed:?} against {kept:?}"));
                }
            }
            f
        })
        .collect();
    for f in fails.iter().take(20) {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// "Same block" by base identifier == by name over every pair of states of a table with
/// several states per block.
#[test]
fn same_block_paths_agree() {
    let Some(pack) = common::data_pack() else { return };
    let mut blocks = generator(&pack, "minecraft:normal", "minecraft:overworld").blocks;
    for s in [
        "minecraft:oak_log[axis=x]",
        "minecraft:oak_log[axis=y]",
        "minecraft:oak_log",
        "minecraft:water[level=3]",
        "minecraft:stone",
        "minecraft:grass_block[snowy=true]",
        "minecraft:cave_air",
    ] {
        blocks.intern(s);
    }
    let n = blocks.len() as u16;
    let (mut same, mut fails) = (0usize, 0usize);
    for a in 0..n {
        for b in 0..n {
            let by_name = blocks.same_block_by_name(a, b);
            same += usize::from(by_name && a != b);
            fails += usize::from(by_name != blocks.same_block_by_base_id(a, b));
        }
    }
    eprintln!("{n} states, {same} pairs of distinct states of the same block");
    assert!(same >= 6, "too few pairs of states of the same block ({same})");
    assert_eq!(fails, 0, "different answers");
}
