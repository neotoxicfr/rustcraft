// Direct read allowed (tests: settings read at launch); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Saving of the structure pieces and placed mobs (G6-F19). Game data required
//! (jar 26.3 and rc-data cache); absent: tests skipped, except `RUSTCRAFT_OPT_REQUIRE=1`.

use std::collections::HashMap;

use rc_proto::nbt::Nbt;

use crate::chunkgen::DimGen;
use crate::decor_region::DecoChunk;
use crate::structure::{Placement, Start};
use crate::structure::tests::{dim, world_data};
use crate::terrain::BiomeCache;

const SEED: i64 = 12345;

/// Seeds tried in order to find a rare structure near the origin.
const SEEDS: [i64; 6] = [SEED, 1, 42, 20261011, -4172144997902289642, 7];

/// First seed that has a start `id` at `radius` chunks or fewer: generator, start, seed.
fn find_seeded(w: &crate::chunkgen::WorldData, id: &str, radius: i32) -> (DimGen, Start, i64) {
    for seed in SEEDS {
        let dg = dim(w, Some(seed));
        if let Some(s) = find(&dg, id, radius) {
            return (dg, s, seed);
        }
    }
    panic!("{id} not found for the seeds {SEEDS:?}")
}

/// First start `id` in a spiral around the origin (radius in chunks).
fn find(dg: &DimGen, id: &str, radius: i32) -> Option<Start> {
    find_with(dg, |s| s == id, radius)
}

fn find_with(dg: &DimGen, ok: impl Fn(&str) -> bool, radius: i32) -> Option<Start> {
    let sg = dg.structures.clone()?;
    for r in 0..radius {
        for x in -r..=r {
            for z in -r..=r {
                if x.abs() != r && z.abs() != r {
                    continue;
                }
                if let Some(s) = dg.structure_starts(x, z).iter().find(|s| ok(&sg.defs[s.structure].id)) {
                    return Some(s.clone());
                }
            }
        }
    }
    None
}

/// Chunks of the start's box (in chunks).
fn box_chunks(s: &Start) -> Vec<(i32, i32)> {
    let (x0, z0, x1, z1) = (s.bbox.min[0] >> 4, s.bbox.min[2] >> 4, s.bbox.max[0] >> 4, s.bbox.max[2] >> 4);
    (x0..=x1).flat_map(|x| (z0..=z1).map(move |z| (x, z))).collect()
}

/// Terrain (with its `structures` compound, like the server) of the given chunks and their neighbours.
fn terrain(dg: &DimGen, centers: &[(i32, i32)]) -> HashMap<(i32, i32), DecoChunk> {
    let mut all: Vec<(i32, i32)> =
        centers.iter().flat_map(|&(x, z)| (0..9).map(move |i| (x + i % 3 - 1, z + i / 3 - 1))).collect();
    all.sort_unstable();
    all.dedup();
    let mut cache = BiomeCache::default();
    dg.prepare_biomes(&mut cache, &all);
    all.iter()
        .map(|&(x, z)| {
            let mut c = dg.terrain(x, z, &cache, None);
            c.structures = Some(dg.structures_nbt(x, z));
            ((x, z), c)
        })
        .collect()
}

fn decorate(dg: &DimGen, chunks: &mut HashMap<(i32, i32), DecoChunk>, (x, z): (i32, i32)) {
    let mut picked: Vec<DecoChunk> =
        (0..9).map(|i| chunks.remove(&(x + i % 3 - 1, z + i / 3 - 1)).expect("neighbouring terrain")).collect();
    {
        let mut it = picked.iter_mut();
        let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
        dg.decorate(refs);
    }
    for c in picked {
        chunks.insert((c.cx, c.cz), c);
    }
}

/// Piece `n` of the start `id` in the `structures` compound.
fn saved_child<'a>(structures: &'a Nbt, id: &str, n: usize) -> &'a Nbt {
    let Some(Nbt::List(ch)) = structures.get("starts").and_then(|s| s.get(id)).and_then(|s| s.get("Children")) else {
        panic!("start {id} missing from the compound");
    };
    &ch[n]
}

fn int(n: &Nbt, k: &str) -> i64 {
    n.get(k).and_then(Nbt::as_i64).unwrap_or_else(|| panic!("field {k} missing: {n:?}"))
}

fn marked(e: &Nbt) -> bool {
    e.get("rc_finalize_structure").is_some()
}

fn id_of(e: &Nbt) -> &str {
    e.get("id").and_then(Nbt::as_str).unwrap_or("?")
}

/// Swamp hut placed: recorded compound with height, moved box, dimensions and
/// flags; witch and cat marked for finalisation; the compound read back in a new
/// generator gives the same compound.
#[test]
fn swamp_hut_saves_moved_box_and_marks_mobs() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let s = find(&dg, "minecraft:swamp_hut", 200).expect("swamp hut");
    let o = (s.cx, s.cz);
    let mut chunks = terrain(&dg, &box_chunks(&s));
    for p in box_chunks(&s) {
        decorate(&dg, &mut chunks, p);
    }
    let saved = chunks[&o].structures.clone().expect("compound");
    let hut = saved_child(&saved, "minecraft:swamp_hut", 0);
    eprintln!("hut {o:?}: {hut:?}");
    assert_eq!(hut.get("id").and_then(Nbt::as_str), Some("minecraft:tesh"));
    let hpos = int(hut, "HPos");
    assert!(hpos >= 0, "height not kept");
    assert_eq!((int(hut, "Width"), int(hut, "Height"), int(hut, "Depth")), (7, 7, 9));
    let Some(Nbt::IntArray(bb)) = hut.get("BB") else { panic!("BB") };
    assert_eq!((i64::from(bb[1]), i64::from(bb[4])), (hpos, hpos + 6), "box not moved");
    assert_eq!((int(hut, "Witch"), int(hut, "Cat")), (1, 1));
    let mobs: Vec<&Nbt> = chunks
        .values()
        .flat_map(|c| c.entities.iter())
        .filter(|e| matches!(id_of(e), "minecraft:witch" | "minecraft:cat"))
        .collect();
    assert_eq!(mobs.len(), 2, "{mobs:?}");
    for e in &mobs {
        assert!(marked(e), "mob not marked: {e:?}");
        // The recorded box contains the mob (spawns and cat variant read it).
        let Some(Nbt::List(pos)) = e.get("Pos") else { panic!("Pos") };
        let Nbt::Double(y) = pos[1] else { panic!("Pos") };
        assert!((f64::from(bb[1])..=f64::from(bb[4])).contains(&y), "mob outside the box: {y}");
    }
    // Re-read: new generator (restart), piece never yet placed.
    let dg2 = dim(w, Some(SEED));
    let (sg2, t2) = (dg2.structures.clone().unwrap(), dg2.structure_terrain().unwrap());
    let fresh = crate::structure_nbt::chunk_structures(&sg2, o.0, o.1, t2, &dg2.blocks);
    assert_eq!(int(saved_child(&fresh, "minecraft:swamp_hut", 0), "HPos"), -1);
    crate::structure_nbt::restore_scattered(&sg2, o.0, o.1, t2, &dg2.blocks, &saved);
    let back = crate::structure_nbt::chunk_structures(&sg2, o.0, o.1, t2, &dg2.blocks);
    assert_eq!(saved_child(&back, "minecraft:swamp_hut", 0), hut);
}

/// Pyramid straddling two chunks, placed in two goes with a restart between them:
/// the second half takes up the height recorded by the first (same blocks as without
/// restart). Without re-read, the second half re-measures the ground and draws another offset.
#[test]
fn desert_pyramid_resumes_after_restart() {
    let Some(w) = world_data() else { return };
    let (dg, s, seed) = find_seeded(w, "minecraft:desert_pyramid", 120);
    let o = (s.cx, s.cz);
    let n = *box_chunks(&s).iter().find(|&&p| p != o).expect("pyramid over several chunks");
    let order = [n, o];
    // Reference: a single generator.
    let mut a = terrain(&dg, &order);
    for p in order {
        decorate(&dg, &mut a, p);
    }
    // Restart between the two placements.
    let dg_b = dim(w, Some(seed));
    let mut b = terrain(&dg_b, &order);
    decorate(&dg_b, &mut b, n);
    let child = saved_child(b[&o].structures.as_ref().unwrap(), "minecraft:desert_pyramid", 0).clone();
    eprintln!("pyramid {o:?} after the first half: {child:?}");
    assert!(int(&child, "HPos") >= 0, "height not recorded in the starting chunk");
    drop(dg_b);
    let dg_c = dim(w, Some(seed));
    decorate(&dg_c, &mut b, o);
    for p in box_chunks(&s) {
        assert!(a[&p].blocks == b[&p].blocks, "chunk {p:?} differs after restart");
    }
    let (ca, cb) = (a[&o].structures.as_ref().unwrap(), b[&o].structures.as_ref().unwrap());
    assert_eq!(saved_child(ca, "minecraft:desert_pyramid", 0), saved_child(cb, "minecraft:desert_pyramid", 0));
}

/// Decorated chunks of the whole box of a start: entities placed.
fn structure_entities(id: &str, radius: i32) -> Option<Vec<Nbt>> {
    let w = world_data()?;
    let (dg, s, _) = find_seeded(w, id, radius);
    let mut chunks = terrain(&dg, &box_chunks(&s));
    for p in box_chunks(&s) {
        decorate(&dg, &mut chunks, p);
    }
    Some(chunks.into_values().flat_map(|c| c.entities).collect())
}

/// Elder guardians of the monument: marked for the game's finalisation.
#[test]
fn monument_elders_marked() {
    let Some(es) = structure_entities("minecraft:monument", 200) else { return };
    let elders: Vec<&Nbt> = es.iter().filter(|e| id_of(e) == "minecraft:elder_guardian").collect();
    assert_eq!(elders.len(), 3);
    assert!(elders.iter().all(|e| marked(e)), "{elders:?}");
}

/// Mansion: all its mobs (illagers and allays) marked; allay groups of 1 to 3.
#[test]
fn mansion_mobs_all_marked() {
    let Some(es) = structure_entities("minecraft:mansion", 160) else { return };
    let mut count: HashMap<&str, usize> = HashMap::new();
    for e in &es {
        *count.entry(id_of(e)).or_default() += 1;
    }
    eprintln!("mansion: {count:?}");
    let mobs: Vec<&Nbt> = es
        .iter()
        .filter(|e| matches!(id_of(e), "minecraft:evoker" | "minecraft:vindicator" | "minecraft:allay"))
        .collect();
    assert!(count.get("minecraft:allay").is_some_and(|n| *n > 0), "no allay: {count:?}");
    assert!(mobs.iter().all(|e| marked(e)), "mansion mob not marked");
}

/// Jigsaw piece: junctions and liquid setting written.
#[test]
fn jigsaw_piece_writes_junctions_and_liquids() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let s = find_with(&dg, |id| id.starts_with("minecraft:village_"), 160).expect("village");
    let n = dg.structures_nbt(s.cx, s.cz);
    let id = dg.structures.as_ref().unwrap().defs[s.structure].id.clone();
    let Some(Nbt::List(ch)) = n.get("starts").and_then(|x| x.get(&id)).and_then(|x| x.get("Children")) else { panic!() };
    let mut junctions = 0;
    for c in ch {
        assert_eq!(c.get("liquid_settings").and_then(Nbt::as_str), Some("apply_waterlogging"));
        let Some(Nbt::List(j)) = c.get("junctions") else { panic!("junctions") };
        junctions += j.len();
        for k in j {
            for f in ["source_x", "source_ground_y", "source_z", "delta_y"] {
                int(k, f);
            }
            assert!(matches!(k.get("dest_proj").and_then(Nbt::as_str), Some("rigid" | "terrain_matching")));
        }
    }
    assert!(junctions > 0, "no junction recorded ({} pieces)", ch.len());
}

/// Exclusion zone also read for the rings: a stronghold near an excluded village disappears.
#[test]
fn rings_honour_exclusion_zone() {
    let Some(w) = world_data() else { return };
    let mut dg = dim(w, Some(SEED));
    let sg = std::sync::Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap();
    let rings = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).unwrap();
    let villages = sg.sets.iter().position(|s| s.id == "minecraft:villages").expect("village set");
    if let Placement::Rings { exclusion, .. } = &mut sg.sets[rings].placement {
        *exclusion = Some(("minecraft:villages".into(), 16));
    }
    let sg = dg.structures.clone().unwrap();
    let t = dg.structure_terrain().unwrap();
    let mut excluded = 0;
    for &(x, z) in sg.ring_positions(rings, t).iter().take(16) {
        let near = (x - 16..=x + 16).any(|a| (z - 16..=z + 16).any(|b| sg.is_structure_chunk(villages, a, b, t)));
        assert_eq!(sg.is_structure_chunk(rings, x, z, t), !near, "stronghold ({x}, {z})");
        excluded += usize::from(near);
    }
    assert!(excluded > 0, "no stronghold excluded: test without effect");
}

/// Density function bounds: values at ±1e6, increasing thresholds, search floor.
#[test]
fn density_bounds_validated() {
    let p = |s: &str| crate::density::parse(&crate::json::parse(s).unwrap());
    assert!(p("1000000").is_ok());
    assert!(p("1000000.5e1").is_err());
    assert!(p(r#"{"type":"minecraft:constant","value":-2e6}"#).is_err());
    assert!(p(r#"{"type":"minecraft:clamp","input":0,"min":-1,"max":2e6}"#).is_err());
    assert!(p(r#"{"type":"minecraft:range_choice","input":0,"min_inclusive":-3e6,"max_exclusive":1,"when_in_range":0,"when_out_of_range":1}"#).is_err());
    let sel = |t: &str| format!(r#"{{"type":"minecraft:interval_select","input":0,"thresholds":{t},"functions":[0,1,2]}}"#);
    assert!(p(&sel("[0,1]")).is_ok());
    assert!(p(&sel("[1,1]")).is_ok());
    assert!(p(&sel("[1,0]")).is_err());
    assert!(p(&sel("[0,2e6]")).is_err());
    let top = |lb: i32| format!(r#"{{"type":"minecraft:find_top_surface","density":0,"upper_bound":0,"lower_bound":{lb},"cell_height":1}}"#);
    assert!(p(&top(-4064)).is_ok() && p(&top(4062)).is_ok());
    assert!(p(&top(-4065)).is_err() && p(&top(4063)).is_err());
    let grad = |v: &str| format!(r#"{{"type":"minecraft:gradient","axis":"y","from_coordinate":0,"to_coordinate":1,"from_value":{v},"to_value":0}}"#);
    assert!(p(&grad("1")).is_ok() && p(&grad("1e7")).is_err());
}

/// Bench (ignored): cost of reading back and rewriting the scattered pieces per chunk, compound
/// without scattered piece (common case) and hut compound.
#[test]
#[ignore = "benchmark"]
fn scattered_sync_cost() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let s = find(&dg, "minecraft:swamp_hut", 200).expect("swamp hut");
    let (sg, t) = (dg.structures.clone().unwrap(), dg.structure_terrain().unwrap());
    let hut = dg.structures_nbt(s.cx, s.cz);
    let empty = dg.structures_nbt(s.cx + 3, s.cz + 3);
    for (name, n0, (cx, cz)) in [("empty", &empty, (s.cx + 3, s.cz + 3)), ("hut", &hut, (s.cx, s.cz))] {
        let mut n = n0.clone();
        let t0 = std::time::Instant::now();
        const N: u32 = 100_000;
        for _ in 0..N {
            crate::structure_nbt::restore_scattered(&sg, cx, cz, t, &dg.blocks, &n);
            crate::structure_nbt::refresh_scattered(&sg, cx, cz, t, &dg.blocks, &mut n);
        }
        eprintln!("{name}: {:.1} ns per chunk (re-read + rewrite)", t0.elapsed().as_nanos() as f64 / f64::from(N));
    }
}