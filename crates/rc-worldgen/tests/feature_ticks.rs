//! Scheduled ticks and post-processing marks placed by the decoration features (springs,
//! geode cracks, ticking flowers, lakes, disks, lichen), compared with the game's behaviour
//! (ticks of a proto-chunk, marking above placed blocks). Game data required
//! (jar 26.3 and rc-data cache): their absence makes the tests fail.
#![allow(clippy::disallowed_methods)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rc_worldgen::chunkgen::{DimGen, GenSpec, SourceSpec, WorldData};
use rc_worldgen::decor_blocks::State;
use rc_worldgen::decor_region::{DecoChunk, FLUID_TICK_NAMES, Region};

fn overworld() -> DimGen {
    let mc = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft")).expect("APPDATA");
    let jar = mc.join("versions").join("26.3").join("26.3.jar");
    assert!(jar.is_file(), "jar 26.3 absent : {}", jar.display());
    let v = rc_data::load_cached("26.3").expect("cache rc-data 26.3");
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    let w = WorldData::load(&jar, Some(&reports), &v).expect("world data");
    let spec = GenSpec::Noise {
        settings: "minecraft:overworld".into(),
        source: SourceSpec::MultiNoise("minecraft:overworld".into()),
    };
    DimGen::new(&w, &spec, "minecraft:overworld", 1).expect("generator")
}

fn state(dg: &DimGen, name: &str) -> State {
    dg.blocks.default_state(dg.blocks.block_id(name).unwrap_or_else(|| panic!("block {name}")))
}

/// 3×3 chunks around (0, 0), filled by `fill(y)`.
fn chunks(dg: &DimGen, fill: impl Fn(i32) -> State) -> Vec<DecoChunk> {
    (0..9)
        .map(|k| {
            let mut c = DecoChunk::new(k % 3 - 1, k / 3 - 1, dg.min_y, dg.height);
            for y in dg.min_y..dg.min_y + dg.height {
                let s = fill(y);
                let i = c.index(0, y, 0);
                c.blocks[i..i + 256].fill(s);
            }
            c
        })
        .collect()
}

fn put(cs: &mut [DecoChunk], p: [i32; 3], s: State) {
    let c = cs.iter_mut().find(|c| c.cx == p[0] >> 4 && c.cz == p[2] >> 4).expect("chunk");
    let i = c.index(p[0], p[1], p[2]);
    c.blocks[i] = s;
}

fn at(cs: &[DecoChunk], p: [i32; 3]) -> State {
    let c = cs.iter().find(|c| c.cx == p[0] >> 4 && c.cz == p[2] >> 4).expect("chunk");
    c.blocks[c.index(p[0], p[1], p[2])]
}

fn place(dg: &DimGen, cs: &mut [DecoChunk], feature: &str, seed: i64, o: [i32; 3]) -> bool {
    let deco = dg.deco.clone().expect("decoration");
    let mut it = cs.iter_mut();
    let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
    let mut r = Region::new(refs, &dg.blocks, dg.sea_level, dg.seed);
    let mut cx = rc_worldgen::decor::Ctx::new(&mut r, &deco);
    let mut rng = rc_worldgen::decor_rng::DecoRng::xoroshiro(seed);
    let f = deco.reg.feature_id(feature).unwrap_or_else(|| panic!("feature {feature}"));
    rc_worldgen::feature::place_feature(&mut cx, f, &mut rng, o)
}

/// Spring: hollow surrounded by four valid blocks and a single opening → fluid placed and immediate
/// tick of the spring's fluid type (water, then lava).
#[test]
fn springs_schedule_their_fluid_tick() {
    let dg = overworld();
    for (feature, rock, kind) in [
        ("minecraft:spring_water", "stone", "minecraft:water"),
        ("minecraft:spring_lava_overworld", "deepslate", "minecraft:lava"),
    ] {
        let rock = state(&dg, rock);
        let mut cs = chunks(&dg, |_| rock);
        let o = [8, 64, 8];
        let air = state(&dg, "air");
        put(&mut cs, o, air);
        put(&mut cs, [8, 64, 9], air);
        assert!(place(&dg, &mut cs, feature, 0, o), "{feature} not placed");
        let ticks: Vec<_> = cs[4].fluid_ticks.iter().map(|&(p, k)| (p, FLUID_TICK_NAMES[usize::from(k)])).collect();
        assert_eq!(ticks, vec![(o, kind)], "{feature}");
        assert!(cs.iter().enumerate().all(|(i, c)| i == 4 || c.fluid_ticks.is_empty()));
    }
}

/// Pale garden flower: closed eyeblossom placed with a block tick (delay 1 requested).
#[test]
fn pale_garden_eyeblossom_schedules_block_tick() {
    let dg = overworld();
    let air = state(&dg, "air");
    let mut cs = chunks(&dg, |_| air);
    let o = [8, 64, 8];
    put(&mut cs, [8, 63, 8], state(&dg, "grass_block"));
    assert!(place(&dg, &mut cs, "minecraft:flower_pale_garden", 0, o));
    let closed = dg.blocks.block_id("closed_eyeblossom").unwrap();
    assert!(dg.blocks.is(at(&cs, o), closed));
    assert_eq!(cs[4].block_ticks, vec![(o, closed, 1)]);
}

/// Lava lake in stone: each emptied cell (cave air) gets a block tick
/// of air, and the two blocks above the highest void of each column are marked.
#[test]
fn lake_air_ticks_and_marks_above() {
    let dg = overworld();
    let stone = state(&dg, "stone");
    let mut cs = chunks(&dg, |_| stone);
    let o = [8, 64, 8];
    assert!(place(&dg, &mut cs, "minecraft:lake_lava", 3, o));
    let cave_air = dg.blocks.block_id("cave_air").unwrap();
    let c = &cs[4];
    assert!(!c.block_ticks.is_empty(), "no air tick");
    let mut top: BTreeMap<(i32, i32), i32> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for &(p, k, d) in &c.block_ticks {
        assert_eq!((k, d), (cave_air, 0), "unexpected tick at {p:?}");
        assert!(dg.blocks.is(at(&cs, p), cave_air), "{p:?} is not empty");
        assert!(seen.insert(p), "duplicate tick at {p:?}");
        let e = top.entry((p[0], p[2])).or_insert(p[1]);
        *e = (*e).max(p[1]);
    }
    let marks: BTreeSet<[i32; 3]> = c.post_process.iter().copied().collect();
    for (&(x, z), &y) in &top {
        for dy in 1..=2 {
            assert!(marks.contains(&[x, y + dy, z]), "({x}, {}, {z}) not marked", y + dy);
        }
    }
    eprintln!("lake: {} air ticks, {} marks", c.block_ticks.len(), c.post_process.len());
}

/// Sand disk under grass: only the grass above the first replaced block of the column
/// is marked (no air above the other columns, nor mark under the top of a run).
#[test]
fn disk_marks_block_above_first_replaced() {
    let dg = overworld();
    let (dirt, air) = (state(&dg, "dirt"), state(&dg, "air"));
    let mut cs = chunks(&dg, |y| if y <= 63 { dirt } else { air });
    put(&mut cs, [8, 64, 8], state(&dg, "short_grass"));
    assert!(place(&dg, &mut cs, "minecraft:disk_sand", 0, [8, 63, 8]));
    let all: Vec<[i32; 3]> = cs.iter().flat_map(|c| c.post_process.iter().copied()).collect();
    assert_eq!(all, vec![[8, 64, 8]]);
}

/// Glow lichen in a one-block cavity: the placed cell is marked (and again at each
/// extension onto another face of the same cell).
#[test]
fn glow_lichen_marks_its_position() {
    let dg = overworld();
    let stone = state(&dg, "stone");
    let mut cs = chunks(&dg, |_| stone);
    let o = [8, 64, 8];
    put(&mut cs, o, state(&dg, "air"));
    assert!(place(&dg, &mut cs, "minecraft:glow_lichen", 0, o));
    let all: Vec<[i32; 3]> = cs.iter().flat_map(|c| c.post_process.iter().copied()).collect();
    assert!(!all.is_empty() && all.iter().all(|&p| p == o), "marks {all:?}");
}

/// Geode whose crack opens onto water: each water next to a crack cell
/// gets a water tick. First pass in stone to find the crack, second with
/// water on the intact cells that border it (the carving does not depend on these cells).
#[test]
fn geode_crack_ticks_neighbouring_fluids() {
    let dg = overworld();
    let stone = state(&dg, "stone");
    let water = state(&dg, "water");
    let o = [8, 0, 8];
    for seed in 0..32 {
        let mut first = chunks(&dg, |_| stone);
        if !place(&dg, &mut first, "minecraft:amethyst_geode", seed, o) {
            continue;
        }
        let mut wet = BTreeSet::new();
        for x in -12..28 {
            for y in -20..20 {
                for z in -12..28 {
                    let p = [x, y, z];
                    if !dg.blocks.is_air(at(&first, p)) {
                        continue;
                    }
                    for d in rc_worldgen::decor_blocks::Dir::ALL {
                        let q = rc_worldgen::decor_region::add(p, d);
                        if at(&first, q) == stone {
                            wet.insert(q);
                        }
                    }
                }
            }
        }
        if wet.is_empty() {
            continue;
        }
        let mut second = chunks(&dg, |_| stone);
        for &q in &wet {
            put(&mut second, q, water);
        }
        assert!(place(&dg, &mut second, "minecraft:amethyst_geode", seed, o));
        let mut ticked = BTreeSet::new();
        for c in &second {
            for &(p, k) in &c.fluid_ticks {
                assert_eq!(FLUID_TICK_NAMES[usize::from(k)], "minecraft:water");
                ticked.insert(p);
            }
        }
        eprintln!("geode (seed {seed}): {} waters at the edge of the crack, {} ticks", wet.len(), ticked.len());
        assert_eq!(ticked, wet);
        return;
    }
    panic!("no cracked geode over 32 seeds");
}

/// Lava lake in dirt: each placed barrier block (dirt turned to stone) that carries a
/// non-empty block makes that block be marked.
#[test]
fn lake_barrier_marks_above() {
    let dg = overworld();
    let (dirt, stone) = (state(&dg, "dirt"), state(&dg, "stone"));
    let mut cs = chunks(&dg, |_| dirt);
    assert!(place(&dg, &mut cs, "minecraft:lake_lava", 3, [8, 64, 8]));
    let marks: BTreeSet<[i32; 3]> = cs.iter().flat_map(|c| c.post_process.iter().copied()).collect();
    let mut checked = 0;
    for x in -16..32 {
        for z in -16..32 {
            for y in 40..90 {
                let (q, up) = ([x, y, z], [x, y + 1, z]);
                if at(&cs, q) != stone || dg.blocks.is_air(at(&cs, up)) {
                    continue;
                }
                assert!(marks.contains(&up), "{up:?} above the barrier {q:?} not marked");
                checked += 1;
            }
        }
    }
    eprintln!("lake: {checked} barriers under a block");
    assert!(checked > 0, "no barrier under a block");
}

/// Fallen trunk on a meadow with two layers of grass: the stump and each placed log make
/// the grass left above them be marked.
#[test]
fn fallen_tree_marks_above_stump_and_logs() {
    let dg = overworld();
    let (dirt, grass, air) = (state(&dg, "dirt"), state(&dg, "short_grass"), state(&dg, "air"));
    let log = dg.blocks.block_id("oak_log").unwrap();
    for seed in 0..32 {
        let mut cs = chunks(&dg, |y| match y {
            ..=63 => dirt,
            64 | 65 => grass,
            _ => air,
        });
        if !place(&dg, &mut cs, "minecraft:fallen_oak_tree", seed, [8, 64, 8]) {
            continue;
        }
        let marks: BTreeSet<[i32; 3]> = cs.iter().flat_map(|c| c.post_process.iter().copied()).collect();
        let mut logs = Vec::new();
        for x in -8..24 {
            for z in -8..24 {
                if dg.blocks.is(at(&cs, [x, 64, z]), log) {
                    logs.push([x, 64, z]);
                }
            }
        }
        if logs.len() < 2 {
            continue;
        }
        for p in &logs {
            let up = [p[0], 65, p[2]];
            if !dg.blocks.is_air(at(&cs, up)) {
                assert!(marks.contains(&up), "{up:?} above the log {p:?} not marked (seed {seed})");
            }
        }
        eprintln!("fallen trunk (seed {seed}): {} logs, {} marks", logs.len(), marks.len());
        return;
    }
    panic!("no complete fallen trunk over 32 seeds");
}

/// Glow lichen in a stone corridor: when it spreads onto another cell, that cell is
/// marked too (every cell that carries lichen is).
#[test]
fn glow_lichen_spread_marks_target() {
    let dg = overworld();
    let stone = state(&dg, "stone");
    let lichen = dg.blocks.block_id("glow_lichen").unwrap();
    let o = [8, 64, 8];
    for seed in 0..64 {
        let mut cs = chunks(&dg, |_| stone);
        for x in 6..=10 {
            put(&mut cs, [x, 64, 8], state(&dg, "air"));
        }
        if !place(&dg, &mut cs, "minecraft:glow_lichen", seed, o) {
            continue;
        }
        let grown: Vec<[i32; 3]> =
            (6..=10).map(|x| [x, 64, 8]).filter(|&p| dg.blocks.is(at(&cs, p), lichen)).collect();
        if !grown.iter().any(|&p| p != o) {
            continue;
        }
        let marks: BTreeSet<[i32; 3]> = cs.iter().flat_map(|c| c.post_process.iter().copied()).collect();
        for p in &grown {
            assert!(marks.contains(p), "lichen at {p:?} not marked (seed {seed})");
        }
        return;
    }
    panic!("no lichen spread onto another cell over 64 seeds");
}

/// Nether fortress: the lava well of the entrance and all the fluid ticks of the chunk that
/// contains it carry the lava kind (the kind comes from the placed state, not from a default kind).
#[test]
fn fortress_lava_well_ticks_lava() {
    use rc_worldgen::structure::Piece;
    use rc_worldgen::structure_coded::CodedPiece;
    use rc_worldgen::structure_fortress::FKind;
    let mc = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft")).expect("APPDATA");
    let jar = mc.join("versions").join("26.3").join("26.3.jar");
    let v = rc_data::load_cached("26.3").expect("cache rc-data 26.3");
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    let w = WorldData::load(&jar, Some(&reports), &v).expect("world data");
    let spec = GenSpec::Noise {
        settings: "minecraft:nether".into(),
        source: SourceSpec::MultiNoise("minecraft:nether".into()),
    };
    let mut dg = DimGen::new(&w, &spec, "minecraft:the_nether", 12345).expect("generator");
    dg.enable_structures(&w).unwrap();
    let sg = dg.structures.clone().unwrap();
    let mut well = None;
    'f: for r in 0..96i32 {
        for x in -r..=r {
            for z in -r..=r {
                if x.abs() != r && z.abs() != r {
                    continue;
                }
                for s in dg.structure_starts(x, z).iter().filter(|s| sg.defs[s.structure].id == "minecraft:fortress") {
                    for p in &s.pieces {
                        if let Piece::Coded { bbox, data, .. } = p
                            && let CodedPiece::Fortress(fp) = &**data
                            && matches!(fp.kind, FKind::Entrance)
                        {
                            well = Some([bbox.min[0] + 6, bbox.min[1] + 5, bbox.min[2] + 6]);
                            break 'f;
                        }
                    }
                }
            }
        }
    }
    let well = well.expect("fortress entrance within a radius of 96 chunks");
    let (cx, cz) = (well[0] >> 4, well[2] >> 4);
    let list: Vec<(i32, i32)> = (0..9).map(|k| (cx + k % 3 - 1, cz + k / 3 - 1)).collect();
    let mut cache = rc_worldgen::terrain::BiomeCache::default();
    dg.prepare_biomes(&mut cache, &list);
    let mut cs: Vec<DecoChunk> = list.iter().map(|&(x, z)| dg.terrain(x, z, &cache, None)).collect();
    {
        let mut it = cs.iter_mut();
        let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
        dg.decorate(refs);
    }
    let lava = dg.blocks.block_id("lava").unwrap();
    assert!(dg.blocks.is(at(&cs, well), lava), "no lava at the well {well:?}");
    let c = &cs[4];
    assert!(c.fluid_ticks.iter().any(|&(p, _)| p == well), "no tick at the well {well:?}");
    for &(p, k) in &c.fluid_ticks {
        let name = FLUID_TICK_NAMES[usize::from(k)];
        assert!(dg.blocks.is(at(&cs, p), lava) && name.ends_with("lava"), "tick {name} at {p:?}");
    }
    eprintln!("fortress: well {well:?}, {} fluid ticks", c.fluid_ticks.len());
}
